use core::mem::{self, MaybeUninit};
use core::slice;

use crate::error::KernelError;
use crate::fs::Stat;
use crate::fs::{BSIZE, Inode};
use crate::param::{MAXOPBLOCKS, NFILE};
use crate::proc::Addr;
use crate::sleeplock::SleepLock;
use crate::spinlock::SpinLock;
use crate::vm::VA;
use crate::{log, proc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    None,
    Pipe { pipe: () },
    Inode { inode: Inode },
    Device { inode: Inode, major: u16 },
}

/// File metadata protected by table-wide spinlock
#[derive(Debug, Clone, Copy)]
pub struct FileMeta {
    pub ref_count: usize,
}

/// Per-file mutable state protected by per-file sleeplock
#[derive(Debug, Clone, Copy)]
pub struct FileInner {
    /// Index into the file table.
    pub readable: bool,
    pub writeable: bool,
    pub r#type: FileType,
    pub offset: u32,
}

impl FileInner {
    const fn new() -> Self {
        Self {
            readable: false,
            writeable: false,
            r#type: FileType::None,
            offset: 0,
        }
    }
}

pub static FILE_TABLE: FileTable = FileTable::new();

/// Global file table
#[derive(Debug)]
pub struct FileTable {
    /// Protects allocation and reference counts
    pub meta: SpinLock<[FileMeta; NFILE]>,
    /// Per-file locks for concurrent access to different files
    pub inner: [SleepLock<FileInner>; NFILE],
}

impl FileTable {
    const fn new() -> Self {
        let meta = {
            let mut array: [MaybeUninit<FileMeta>; NFILE] =
                unsafe { MaybeUninit::uninit().assume_init() };

            let mut i = 0;
            while i < NFILE {
                array[i] = MaybeUninit::new(FileMeta { ref_count: 0 });
                i += 1;
            }

            SpinLock::new(
                unsafe {
                    mem::transmute::<[MaybeUninit<FileMeta>; NFILE], [FileMeta; NFILE]>(array)
                },
                "filetable",
            )
        };

        let inner = {
            let mut array: [MaybeUninit<SleepLock<FileInner>>; NFILE] =
                unsafe { MaybeUninit::uninit().assume_init() };

            let mut i = 0;
            while i < NFILE {
                array[i] = MaybeUninit::new(SleepLock::new(FileInner::new(), "file"));
                i += 1;
            }

            unsafe {
                mem::transmute::<
                    [MaybeUninit<SleepLock<FileInner>>; NFILE],
                    [SleepLock<FileInner>; NFILE],
                >(array)
            }
        };

        Self { meta, inner }
    }
}

/// File handle, just an index into the `FileTable`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct File {
    pub id: usize,
}

impl File {
    /// Allocates a file structure.
    pub fn alloc() -> Result<Self, KernelError> {
        let mut meta = FILE_TABLE.meta.lock();

        for (i, meta) in meta.iter_mut().enumerate() {
            if meta.ref_count == 0 {
                meta.ref_count = 1;

                return Ok(Self { id: i });
            }
        }

        Err(KernelError::Fs)
    }

    /// Incremets the reference count for the file.
    pub fn dup(&mut self) -> Self {
        let meta = &mut FILE_TABLE.meta.lock()[self.id];

        assert!(meta.ref_count >= 1, "filedup");

        meta.ref_count += 1;

        *self
    }

    /// Decrements the reference count and closes the file if it reaches 0.
    pub fn close(&mut self) {
        let mut meta_guard = FILE_TABLE.meta.lock();
        let meta = &mut meta_guard[self.id];

        assert!(meta.ref_count >= 1, "fileclose");

        meta.ref_count -= 1;
        if meta.ref_count > 0 {
            return;
        }

        let inner_copy = {
            let mut inner = FILE_TABLE.inner[self.id].lock();
            // copy inner before resetting fields
            let copy = *inner;

            meta.ref_count = 0;
            inner.r#type = FileType::None;

            drop(meta_guard);
            copy
        }; // drop both inner and meta locks

        match inner_copy.r#type {
            FileType::None => {}
            FileType::Pipe { pipe: _ } => {
                todo!("pipeclose");
            }
            FileType::Inode { inode } => {
                log::begin_op();
                inode.put();
                log::end_op();
            }
            FileType::Device { inode, major: _ } => {
                log::begin_op();
                inode.put();
                log::end_op();
            }
        }
    }

    /// Gets metadata about file.
    pub fn stat(&self, addr: VA) -> Result<(), KernelError> {
        let file_inner = FILE_TABLE.inner[self.id].lock();

        match file_inner.r#type {
            FileType::Inode { inode } => {
                let inode_inner = inode.lock();
                let stat = inode.stat(&inode_inner);
                inode.unlock(inode_inner);

                let src = unsafe {
                    slice::from_raw_parts(&stat as *const _ as *const u8, mem::size_of::<Stat>())
                };
                proc::copy_out(src, Addr::User(addr))?;

                Ok(())
            }
            FileType::Device { inode, major } => {
                unimplemented!()
            }
            _ => Err(KernelError::Fs),
        }
    }

    /// Reads from file.
    pub fn read(&self, addr: VA, n: usize) -> Result<usize, KernelError> {
        let mut file_inner = FILE_TABLE.inner[self.id].lock();

        if !file_inner.readable {
            return Err(KernelError::Fs);
        }

        let mut read = 0;

        match file_inner.r#type {
            FileType::None => panic!("fileread"),
            FileType::Pipe { pipe: _ } => {
                unimplemented!()
            }
            FileType::Inode { inode } => {
                let mut inode_inner = inode.lock();
                let dst = unsafe { slice::from_raw_parts_mut(addr.as_mut_ptr(), n) };
                if let Ok(r) = inode.read(&mut inode_inner, file_inner.offset, dst) {
                    file_inner.offset += r;
                    read += r;
                }
                inode.unlock(inode_inner);
            }
            FileType::Device { inode: _, major: _ } => {
                unimplemented!()
            }
        }

        Ok(read as usize)
    }

    /// Writes to a file.
    pub fn write(&mut self, addr: VA, n: usize) -> Result<usize, KernelError> {
        let mut file_inner = FILE_TABLE.inner[self.id].lock();

        if !file_inner.writeable {
            return Err(KernelError::Fs);
        }

        match file_inner.r#type {
            FileType::None => panic!("filewrite"),
            FileType::Pipe { pipe: _ } => {
                unimplemented!()
            }
            FileType::Inode { inode } => {
                // write a few block at a time to avoid exceeding the maximum log transaction size,
                // including inode, indirect block, allocation blocks, and 2 block of slop for
                // non-aligned writes.
                let max = ((MAXOPBLOCKS - 1 - 1 - 2) / 2) * BSIZE;
                let mut i = 0;

                while i < n {
                    let n1 = (n - i).min(max);

                    log::begin_op();
                    let mut inode_inner = inode.lock();

                    let src = unsafe { slice::from_raw_parts(addr.as_usize() as *const u8, n1) };
                    let w = inode.write(&mut inode_inner, file_inner.offset, src);

                    if let Ok(w) = w {
                        file_inner.offset += w;
                    }

                    inode.unlock(inode_inner);
                    log::end_op();

                    if w.is_err() {
                        break;
                    }

                    i += w.unwrap() as usize;
                }

                if i == n { Ok(n) } else { Err(KernelError::Fs) }
            }
            FileType::Device { inode: _, major: _ } => {
                unimplemented!()
            }
        }
    }
}
