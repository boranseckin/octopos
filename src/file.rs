use core::mem::{self, MaybeUninit};
use core::slice;

use crate::console::Console;
use crate::error::KernelError;
use crate::fs::Stat;
use crate::fs::{BSIZE, Inode};
use crate::log;
use crate::param::{MAXOPBLOCKS, NDEV, NFILE};
use crate::proc;
use crate::sleeplock::SleepLock;
use crate::spinlock::SpinLock;
use crate::syscall::SyscallError;
use crate::vm::VA;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileType {
    None,
    Pipe { pipe: () },
    Inode { inode: Inode },
    Device { inode: Inode, major: u16 },
}

/// File metadata protected by table-wide spinlock
#[derive(Debug, Clone)]
pub struct FileMeta {
    pub ref_count: usize,
}

/// Per-file mutable state protected by per-file sleeplock
#[derive(Debug, Clone)]
pub struct FileInner {
    /// Index into the file table.
    pub readable: bool,
    pub writeable: bool,
    pub r#type: FileType,
    pub offset: u32,
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
                array[i] = MaybeUninit::new(SleepLock::new(
                    FileInner {
                        readable: false,
                        writeable: false,
                        r#type: FileType::None,
                        offset: 0,
                    },
                    "file",
                ));
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

        self.clone()
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
            let copy = inner.clone();

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
            FileType::Inode { inode } | FileType::Device { inode, .. } => {
                log::begin_op();
                inode.put();
                log::end_op();
            }
        }
    }

    /// Gets metadata about file.
    pub fn stat(&self, addr: VA) -> Result<(), KernelError> {
        let file_inner = FILE_TABLE.inner[self.id].lock();

        match &file_inner.r#type {
            FileType::Inode { inode } | FileType::Device { inode, .. } => {
                let inode_inner = inode.lock();
                let stat = inode.stat(&inode_inner);
                inode.unlock(inode_inner);

                let src = unsafe {
                    slice::from_raw_parts(&stat as *const _ as *const u8, mem::size_of::<Stat>())
                };
                proc::copy_out_user(src, addr)?;

                Ok(())
            }
            _ => Err(KernelError::Fs),
        }
    }

    /// Reads from file.
    pub fn read(&self, addr: VA, n: usize) -> Result<usize, SyscallError> {
        let mut file_inner = FILE_TABLE.inner[self.id].lock();

        if !file_inner.readable {
            return Err(SyscallError::Read);
        }

        match &mut file_inner.r#type {
            FileType::None => panic!("fileread"),
            FileType::Pipe { pipe: _ } => {
                unimplemented!()
            }
            FileType::Inode { inode } => {
                let inode = inode.clone();
                let mut inode_inner = inode.lock();

                let dst = unsafe { slice::from_raw_parts_mut(addr.as_mut_ptr(), n) };
                let read = inode.read(&mut inode_inner, file_inner.offset, dst, true);

                if let Ok(read) = read {
                    file_inner.offset += read;
                }

                inode.unlock(inode_inner);

                if let Ok(read) = read {
                    Ok(read as usize)
                } else {
                    Err(SyscallError::Read)
                }
            }
            FileType::Device { inode: _, major } => match &DEVICES[*major as usize] {
                Some(dev) => (dev.read)(addr, n),
                None => Err(SyscallError::Read),
            },
        }
    }

    /// Writes to a file.
    pub fn write(&mut self, addr: VA, n: usize) -> Result<usize, SyscallError> {
        let mut file_inner = FILE_TABLE.inner[self.id].lock();

        if !file_inner.writeable {
            return Err(SyscallError::Write);
        }

        match &mut file_inner.r#type {
            FileType::None => panic!("filewrite"),

            FileType::Pipe { pipe: _ } => {
                unimplemented!()
            }

            FileType::Inode { inode } => {
                let inode = inode.clone();

                // write a few block at a time to avoid exceeding the maximum log transaction size,
                // including inode, indirect block, allocation blocks, and 2 block of slop for
                // non-aligned writes.
                let max = ((MAXOPBLOCKS - 1 - 1 - 2) / 2) * BSIZE;
                let mut i = 0;

                while i < n {
                    let n1 = (n - i).min(max);

                    log::begin_op();
                    let mut inode_inner = inode.lock();

                    let src =
                        unsafe { slice::from_raw_parts((addr.as_usize() + i) as *const u8, n1) };
                    let write = inode.write(&mut inode_inner, file_inner.offset, src, true);

                    if let Ok(w) = write {
                        file_inner.offset += w;
                    }

                    inode.unlock(inode_inner);
                    log::end_op();

                    if write.is_err() {
                        break;
                    }

                    i += write.unwrap() as usize;
                }

                if i == n {
                    Ok(n)
                } else {
                    Err(SyscallError::Write)
                }
            }

            FileType::Device { inode: _, major } => match &DEVICES[*major as usize] {
                Some(dev) => (dev.write)(addr, n),
                None => Err(SyscallError::Write),
            },
        }
    }

    /// Open file flags
    pub const O_RDONLY: i32 = 0x000;
    pub const O_WRONLY: i32 = 0x001;
    pub const O_RDWR: i32 = 0x002;
    pub const O_CREATE: i32 = 0x200;
    pub const O_TRUNC: i32 = 0x400;
}

/// Device interface
#[derive(Debug, Clone, Copy)]
pub struct Device {
    pub read: fn(addr: VA, n: usize) -> Result<usize, SyscallError>,
    pub write: fn(addr: VA, n: usize) -> Result<usize, SyscallError>,
}

/// Console device major number
pub const CONSOLE: usize = 1;

/// Device table
pub static DEVICES: [Option<Device>; NDEV] = {
    let mut devices = [None; NDEV];
    devices[CONSOLE] = Some(Device {
        read: Console::read,
        write: Console::write,
    });
    devices
};

/// TEMPORARY console setup from kernel
pub fn setup_console_fds() {
    let proc = crate::proc::CPU_POOL.current_proc().unwrap();
    let data = unsafe { proc.data_mut() };

    // Allocate a file for console device
    let mut file = File::alloc().unwrap();
    {
        let mut inner = FILE_TABLE.inner[file.id].lock();
        inner.readable = true;
        inner.writeable = true;
        inner.r#type = FileType::Device {
            inode: Inode {
                id: 0,
                dev: 0,
                inum: 0,
            },
            major: CONSOLE as u16,
        };
    }

    // fd 0 = stdin, fd 1 = stdout, fd 2 = stderr
    data.open_files[1] = Some(file.dup());
    data.open_files[2] = Some(file.dup());
    data.open_files[0] = Some(file);
}
