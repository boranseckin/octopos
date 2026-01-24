use core::mem::{self, MaybeUninit};
use core::ptr;
use core::slice;

use crate::buf::{BCACHE, Buf};
use crate::error::KernelError;
use crate::param::{NINODE, ROOTDEV};
use crate::proc::{Addr, CPU_POOL};
use crate::sleeplock::{SleepLock, SleepLockGuard};
use crate::spinlock::SpinLock;
use crate::sync::OnceLock;
use crate::vm::VA;
use crate::{log, println, proc};

/// File system magic number
pub const FSMAGIC: u32 = 0x10203040;

/// Root inode number
pub const ROOTINO: u32 = 1;
/// Block size
pub const BSIZE: usize = 1024;
/// Number of direct block addresses in inode
pub const NDIRECT: usize = 12;
/// Number of indirect block addresses in inode
pub const NINDIRECT: usize = BSIZE / size_of::<u32>();
/// Max file size (blocks)
pub const MAXFILE: usize = NDIRECT + NINDIRECT;

/// Inodes per block
pub const IPB: u32 = (BSIZE / size_of::<DiskInode>()) as u32;
/// Bitmap bits per block
pub const BPB: u32 = BSIZE as u32 * 8;
/// Directory entry name size
pub const DIRSIZE: usize = 14;

pub static SB: OnceLock<SuperBlock> = OnceLock::new();

/// On-disk superblock (read at boot)
#[repr(C)]
#[derive(Debug, Clone)]
pub struct SuperBlock {
    /// Must be `FSMAGIC`
    pub magic: u32,
    /// Size of file system image (blocks)
    pub size: u32,
    /// Number of data blocks
    pub nblocks: u32,
    /// Number of inodes
    pub ninodes: u32,
    /// Number of log blocks
    pub nlogs: u32,
    /// Block number of first log block
    pub logstart: u32,
    /// Block number of first inode block
    pub inodestart: u32,
    /// Block number of first free map block
    pub bmapstart: u32,
}

impl SuperBlock {
    /// Reads the superblock from disk and initializes the global `SB`.
    fn initialize(dev: u32) {
        let buf = BCACHE.read(dev, 1); // superblock is at block 1
        let sb = unsafe { ptr::read_unaligned(buf.data().as_ptr() as *const SuperBlock) };
        BCACHE.release(buf);

        assert_eq!(sb.magic, FSMAGIC, "invalid file system");

        SB.initialize(|| Ok::<_, ()>(sb));
    }
}

/// Initialize the file system.
pub fn init(dev: u32) {
    SuperBlock::initialize(dev);
    log::init(dev, SB.get().unwrap());
    Inode::reclaim(dev);
}

/// A disk block.
/// This is a wrapper around the block number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block(pub u32);

impl Block {
    /// Zeros out the disk block.
    fn zero(&mut self, dev: u32) {
        let mut buf = BCACHE.read(dev, self.0);
        buf.data_mut().fill(0);
        log::write(&buf);
        BCACHE.release(buf);
    }

    /// Allocates a zeroed disk block.
    pub fn alloc(dev: u32) -> Result<Self, KernelError> {
        let sb = SB.get().unwrap();

        for b in (0..sb.size).step_by(BPB as usize) {
            let mut buf = BCACHE.read(dev, sb.bmapstart + (b / BPB));

            for bi in 0..BPB {
                if b + bi >= sb.size {
                    break;
                }

                let m = 1u8 << (bi % 8);
                if buf.data()[bi as usize / 8] & m == 0 {
                    // block is free, mark it as in use
                    buf.data_mut()[bi as usize / 8] |= m;
                    log::write(&buf);
                    BCACHE.release(buf);

                    let mut block = Self(b + bi);
                    block.zero(dev);

                    return Ok(block);
                }
            }

            BCACHE.release(buf);
        }

        // panic!("balloc: out of blocks");
        Err(KernelError::Fs)
    }

    /// Frees a disk block.
    pub fn free(self, dev: u32) {
        let sb = SB.get().unwrap();
        let mut buf = BCACHE.read(dev, sb.bmapstart + (self.0 / BPB));
        let bi = self.0 % BPB;
        let m = 1u8 << (bi % 8);

        if buf.data()[bi as usize / 8] & m == 0 {
            panic!("bfree: block already free");
        }

        buf.data_mut()[bi as usize / 8] &= !m;
        log::write(&buf);
        BCACHE.release(buf);
    }
}

/// Inode types
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeType {
    Free = 0,
    Directory = 1,
    File = 2,
    Device = 3,
}

/// On-disk inode structure
#[repr(C)]
#[derive(Debug)]
pub struct DiskInode {
    /// File type
    pub r#type: InodeType,
    /// Major device number
    pub major: u16,
    /// Minor device number
    pub minor: u16,
    /// Number of links to inode in file system
    pub nlink: u16,
    // Size of file (bytes)
    pub size: u32,
    // Data block addresses
    pub addrs: [u32; NDIRECT + 1],
}

impl DiskInode {
    /// Returns a mutable reference to the `DiskInode` with number `inum` in the given buffer.
    ///
    /// # Safety
    /// The caller must ensure that `buf` contains the block that holds the inode with number `inum`.
    pub unsafe fn from_buf(buf: &mut Buf, inum: u32) -> &'static mut Self {
        unsafe {
            &mut *(buf
                .data_mut()
                .as_mut_ptr()
                .add((inum % IPB) as usize * size_of::<DiskInode>())
                as *mut DiskInode)
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Stat {
    pub dev: u32,
    pub ino: u32,
    pub r#type: InodeType,
    pub nlink: u16,
    pub size: u64,
}

/// Cached inode data, protected by sleeplock
#[derive(Debug)]
pub struct InodeInner {
    /// Indicates whether inode has been read from disk
    pub valid: bool,
    pub r#type: InodeType,
    pub major: u16,
    pub minor: u16,
    pub nlink: u16,
    pub size: u32,
    pub addrs: [u32; NDIRECT + 1],
}

impl InodeInner {
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            valid: false,
            r#type: InodeType::Free,
            major: 0,
            minor: 0,
            nlink: 0,
            size: 0,
            addrs: [0; NDIRECT + 1],
        }
    }
}

/// Metadata about an inode, protected by SpinLock
pub struct InodeMeta {
    /// Device number
    pub dev: u32,
    /// Inode number
    pub inum: u32,
    /// Reference Count
    pub r#ref: u32,
}

impl InodeMeta {
    #[allow(clippy::new_without_default)]
    pub const fn new() -> Self {
        Self {
            dev: 0,
            inum: 0,
            r#ref: 0,
        }
    }
}

/// In-memory inode structure
/// `id` is the index to the actual data in the inode table.
/// Also holds device and inode numbers for quick lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inode {
    /// Inode table index
    pub id: usize,
    /// Device number
    pub dev: u32,
    /// Inode number
    pub inum: u32,
}

impl Inode {
    pub const fn new(id: usize, dev: u32, inum: u32) -> Self {
        Self { id, dev, inum }
    }
}

pub static INODE_TABLE: InodeTable = InodeTable::new();

pub struct InodeTable {
    meta: SpinLock<[InodeMeta; NINODE]>,
    inner: [SleepLock<InodeInner>; NINODE],
}

impl InodeTable {
    const fn new() -> Self {
        let meta = {
            let mut array: [MaybeUninit<InodeMeta>; NINODE] =
                unsafe { MaybeUninit::uninit().assume_init() };

            let mut i = 0;
            while i < NINODE {
                array[i] = MaybeUninit::new(InodeMeta::new());
                i += 1;
            }

            SpinLock::new(
                unsafe {
                    mem::transmute::<[MaybeUninit<InodeMeta>; NINODE], [InodeMeta; NINODE]>(array)
                },
                "itable",
            )
        };

        let inner = {
            let mut array: [MaybeUninit<SleepLock<InodeInner>>; NINODE] =
                unsafe { MaybeUninit::uninit().assume_init() };

            let mut i = 0;
            while i < NINODE {
                array[i] = MaybeUninit::new(SleepLock::new(InodeInner::new(), "inode"));
                i += 1;
            }

            unsafe {
                mem::transmute::<
                    [MaybeUninit<SleepLock<InodeInner>>; NINODE],
                    [SleepLock<InodeInner>; NINODE],
                >(array)
            }
        };

        Self { meta, inner }
    }
}

impl Inode {
    /// Allocates an inode on device `dev`.
    /// Marks it allocated by giving it type `type`.
    /// Returns an unlocked but allocated and referenced inode or error.
    pub fn alloc(dev: u32, r#type: InodeType) -> Result<Self, KernelError> {
        let sb = SB.get().unwrap();

        for inum in 1..sb.ninodes {
            let mut buf = BCACHE.read(dev, sb.inodestart + inum / IPB);
            let dinode = unsafe { DiskInode::from_buf(&mut buf, inum) };

            if dinode.r#type == InodeType::Free {
                dinode.r#type = r#type;
                log::write(&buf);
                BCACHE.release(buf);
                return Self::get(dev, inum);
            }

            BCACHE.release(buf);
        }

        Err(KernelError::Fs)
    }

    /// Finds the inode with number `inum` on device `dev` and returns the in-memory copy.
    /// Does not lock the inode and does not read it from disk.
    pub fn get(dev: u32, inum: u32) -> Result<Self, KernelError> {
        let mut meta = INODE_TABLE.meta.lock();

        let mut empty = None;

        for (id, inode) in meta.iter_mut().enumerate() {
            if inode.r#ref > 0 && inode.dev == dev && inode.inum == inum {
                inode.r#ref += 1;
                return Ok(Self { id, dev, inum });
            }

            if empty.is_none() && inode.r#ref == 0 {
                empty = Some(id);
            }
        }

        if let Some(id) = empty {
            let inode = &mut meta[id];
            inode.dev = dev;
            inode.inum = inum;
            inode.r#ref = 1;

            // # Safety: We have exclusive access to this inode since its ref count is 0.
            let inner = unsafe { INODE_TABLE.inner[id].get_mut_unchecked() };
            inner.valid = false;

            Ok(Self { id, dev, inum })
        } else {
            Err(KernelError::Fs)
        }
    }

    /// Copies a modified in-memory inode to disk.
    /// Must be called after every change to an `Inode` field that lives on disk.
    pub fn update(&self, inner: &SleepLockGuard<'_, InodeInner>) {
        let sb = SB.get().unwrap();

        let mut buf = BCACHE.read(self.dev, sb.inodestart + (self.inum / IPB));
        let dinode = unsafe { DiskInode::from_buf(&mut buf, self.inum) };

        dinode.r#type = inner.r#type;
        dinode.major = inner.major;
        dinode.minor = inner.minor;
        dinode.nlink = inner.nlink;
        dinode.size = inner.size;
        dinode.addrs.copy_from_slice(&inner.addrs);

        log::write(&buf);
        BCACHE.release(buf);
    }

    /// Increments reference count for `inode`.
    /// Returns `Inode` to enable `inode = idup(inode1)` idiom.
    pub fn dup(&self) -> Self {
        let mut meta = INODE_TABLE.meta.lock();
        meta[self.id].r#ref += 1;
        self.clone()
    }

    /// Locks the given `inode`. The lifetime of the lock is static since it comes from the table.
    /// Reads the inode from disk if necessary.
    pub fn lock(&self) -> SleepLockGuard<'static, InodeInner> {
        let sb = SB.get().unwrap();

        let mut inner = INODE_TABLE.inner[self.id].lock();

        if !inner.valid {
            let mut buf = BCACHE.read(self.dev, sb.inodestart + (self.inum / IPB));
            let dinode = unsafe { DiskInode::from_buf(&mut buf, self.inum) };

            inner.r#type = dinode.r#type;
            inner.major = dinode.major;
            inner.minor = dinode.minor;
            inner.nlink = dinode.nlink;
            inner.size = dinode.size;
            inner.addrs.copy_from_slice(&dinode.addrs);

            BCACHE.release(buf);

            inner.valid = true;
            assert_ne!(inner.r#type, InodeType::Free, "ilock: no type");
        }

        inner
    }

    /// Unlocks the given `inode`.
    pub fn unlock(&self, guard: SleepLockGuard<'static, InodeInner>) {
        drop(guard);
    }

    /// Drops a reference to an in-memory inode.
    /// If that was the last reference, the inode table entry can be recycled.
    /// If that was the last reference and the inode has no links to it, free the inode (and its
    /// content) on disk.
    /// All calls to `iput()` must be inside a transaction in case it has to free the inode.
    pub fn put(mut self) {
        let mut meta = INODE_TABLE.meta.lock();

        if meta[self.id].r#ref == 1 {
            // We are acquiring sleeplock while spinlock is active (interrupts disabled).
            // This is normally problematic since sleeplock can sleep and never wake up but,
            // ref == 1 means no other process can have ip locked, so the inner lock won't block.
            let mut inner = INODE_TABLE.inner[self.id].lock();

            if inner.valid && inner.nlink == 0 {
                // inode has no links and no other refereneces: truncate and free

                drop(meta);

                self.trunc(&mut inner);
                inner.r#type = InodeType::Free;
                self.update(&inner);
                inner.valid = false;

                drop(inner);

                // reacquire meta
                meta = INODE_TABLE.meta.lock();
            }
        }

        meta[self.id].r#ref -= 1;
    }

    /// Common idiom: `unlock()`, then `put()`
    pub fn unlock_put(self, guard: SleepLockGuard<'static, InodeInner>) {
        self.unlock(guard);
        self.put();
    }

    /// Reclaims orphaned inodes on device `dev`.
    /// Called at file system initialization.
    pub fn reclaim(dev: u32) {
        let sb = SB.get().unwrap();

        for inum in 1..sb.ninodes {
            let mut buf = BCACHE.read(dev, sb.inodestart + (inum / IPB));
            let dinode = unsafe { DiskInode::from_buf(&mut buf, inum) };

            if dinode.r#type != InodeType::Free && dinode.nlink == 0 {
                // this is an orphaned inode
                println!("ireclaim: orphaned inode {}", inum);

                let inode = Inode::get(dev, inum).expect("ireclaim: get inode failed");

                log::begin_op();
                let guard = inode.lock();
                inode.unlock(guard);
                inode.put();
                log::end_op();
            }

            BCACHE.release(buf);
        }
    }

    /// Truncates inode (discard contents).
    pub fn trunc(&mut self, inner: &mut SleepLockGuard<'_, InodeInner>) {
        for i in 0..NDIRECT {
            if inner.addrs[i] != 0 {
                let block = Block(inner.addrs[i]);
                block.free(self.dev);
                inner.addrs[i] = 0;
            }
        }

        if inner.addrs[NDIRECT] != 0 {
            let buf = BCACHE.read(self.dev, inner.addrs[NDIRECT]);
            let array =
                unsafe { slice::from_raw_parts(buf.data().as_ptr() as *const u32, NINDIRECT) };

            for block in array {
                if *block != 0 {
                    let b = Block(*block);
                    b.free(self.dev);
                }
            }

            BCACHE.release(buf);
            let block = Block(inner.addrs[NDIRECT]);
            block.free(self.dev);
            inner.addrs[NDIRECT] = 0;
        }

        inner.size = 0;
        self.update(inner);
    }

    /// Returns the disk block address of the nth block in `inode`.
    /// If there is no such block, allocates one.
    pub fn map(
        &self,
        inner: &mut SleepLockGuard<'_, InodeInner>,
        block_no: u32,
    ) -> Result<u32, KernelError> {
        let mut block_no = block_no as usize;

        if block_no < NDIRECT {
            let addr = &mut inner.addrs[block_no];

            if *addr == 0 {
                let block = Block::alloc(self.dev)?;
                *addr = block.0;
            }

            return Ok(*addr);
        }

        block_no -= NDIRECT;

        if block_no < NINDIRECT {
            // load indiret block, allocating if necessary
            let in_block_no = &mut inner.addrs[NDIRECT];

            if *in_block_no == 0 {
                let block = Block::alloc(self.dev)?;
                *in_block_no = block.0;
            }

            let mut buf = BCACHE.read(self.dev, *in_block_no);
            let in_block = unsafe {
                slice::from_raw_parts_mut(buf.data_mut().as_mut_ptr() as *mut u32, NINDIRECT)
            };

            let addr = &mut in_block[block_no];
            if *addr == 0 {
                let block = Block::alloc(self.dev)?;

                *addr = block.0;
                log::write(&buf);
            }

            BCACHE.release(buf);

            return Ok(*addr);
        }

        // panic block out of range
        Err(KernelError::Fs)
    }

    pub fn stat(&self, inner: &SleepLockGuard<'_, InodeInner>) -> Stat {
        Stat {
            dev: self.dev,
            r#type: inner.r#type,
            nlink: inner.nlink,
            size: inner.size as u64,
            ino: self.inum,
        }
    }

    /// Reads data from inode.
    // TODO: Only handles user virtual addresses.
    pub fn read(
        &self,
        inner: &mut SleepLockGuard<'_, InodeInner>,
        offset: u32,
        dst: &mut [u8],
    ) -> Result<u32, KernelError> {
        let mut dst = dst;
        let mut n = dst.len() as u32;
        let mut offset = offset;

        if offset > inner.size || offset.checked_add(n).is_none() {
            return Err(KernelError::Fs);
        }

        if offset + n > inner.size {
            n = inner.size - offset;
        }

        let mut total = 0;

        while total < n {
            if let Ok(addr) = self.map(inner, offset / BSIZE as u32) {
                let buf = BCACHE.read(self.dev, addr);

                let m = (n - total).min(BSIZE as u32 - offset % BSIZE as u32);

                let src = &buf.data()[(offset as usize % BSIZE)..][..m as usize];
                let dst_va = VA::from(dst.as_mut_ptr() as usize);

                if proc::copy_out(src, Addr::User(dst_va)).is_err() {
                    BCACHE.release(buf);
                    return Err(KernelError::Fs);
                }

                BCACHE.release(buf);

                total += m;
                offset += m;
                dst = &mut dst[m as usize..];
            } else {
                break;
            }
        }

        Ok(total)
    }

    pub fn write(
        &self,
        inner: &mut SleepLockGuard<'_, InodeInner>,
        offset: u32,
        src: &[u8],
    ) -> Result<u32, KernelError> {
        let mut src = src;
        let n = src.len() as u32;
        let mut offset = offset;

        if offset > inner.size || offset.checked_add(n).is_none() {
            return Err(KernelError::Fs);
        }

        if offset + n > (MAXFILE * BSIZE) as u32 {
            return Err(KernelError::Fs);
        }

        let mut total = 0;

        while total < n {
            if let Ok(addr) = self.map(inner, offset / BSIZE as u32) {
                let mut buf = BCACHE.read(self.dev, addr);
                let m = (n - total).min(BSIZE as u32 - (offset % BSIZE as u32));

                let src_va = VA::from(src.as_ptr() as usize);
                let dst = &mut buf.data_mut()[(offset as usize % BSIZE)..][..m as usize];

                if proc::copy_in(Addr::User(src_va), dst).is_err() {
                    BCACHE.release(buf);
                    break;
                }

                log::write(&buf);
                BCACHE.release(buf);

                total += m;
                offset += m;
                src = &src[m as usize..];
            } else {
                break;
            }
        }

        if offset > inner.size {
            inner.size = offset;
        }

        // write the inode back to disk even if the size didn't change because the loop above might
        // have called `map()` and added a new block to `addrs[]`.
        self.update(inner);

        Ok(total)
    }

    pub fn create(
        path: &Path,
        r#type: InodeType,
        major: u16,
        minor: u16,
    ) -> Result<(Self, SleepLockGuard<'static, InodeInner>), KernelError> {
        let (parent, name) = path.resolve_parent()?;

        let mut parent_inner = parent.lock();

        // check if the file already exists
        if let Ok((_, inode)) = Directory::lookup(&parent, &mut parent_inner, name) {
            parent.unlock_put(parent_inner);

            let inode_inner = inode.lock();

            // check type matches
            if r#type == InodeType::File
                && (inode_inner.r#type == InodeType::File
                    || inode_inner.r#type == InodeType::Device)
            {
                return Ok((inode, inode_inner));
            }

            // type mismatch
            inode.unlock_put(inode_inner);
            return Err(KernelError::Fs);
        };

        let inode = match Self::alloc(parent.dev, r#type) {
            Ok(v) => v,
            Err(e) => {
                parent.unlock_put(parent_inner);
                return Err(e);
            }
        };

        let mut inode_inner = inode.lock();
        inode_inner.major = major;
        inode_inner.minor = minor;
        inode_inner.nlink = 1;
        inode.update(&inode_inner);

        // create `.` and `..` entries if it is a directory
        // no inode.nlink += 1 for `.` to avoid cyclic ref count
        if r#type == InodeType::Directory
            && (Directory::link(&inode, &mut inode_inner, ".", inode.inum as u16).is_err()
                || Directory::link(&inode, &mut inode_inner, "..", inode.inum as u16).is_err())
        {
            // fail
            inode_inner.nlink = 0;
            inode.update(&inode_inner);
            inode.unlock_put(inode_inner);
            parent.unlock_put(parent_inner);
            return Err(KernelError::Fs);
        }

        if Directory::link(&parent, &mut parent_inner, name, inode.inum as u16).is_err() {
            // fail
            inode_inner.nlink = 0;
            inode.update(&inode_inner);
            inode.unlock_put(inode_inner);
            parent.unlock_put(parent_inner);
            return Err(KernelError::Fs);
        }

        if r#type == InodeType::Directory {
            // success is now guarenteed
            parent_inner.nlink += 1;
            parent.update(&parent_inner);
        }

        parent.unlock_put(parent_inner);

        Ok((inode, inode_inner))
    }
}

#[repr(C)]
#[derive(Debug, Clone)]
pub struct Directory {
    pub inum: u16,
    pub name: [u8; DIRSIZE],
}

impl Directory {
    pub const SIZE: usize = size_of::<Self>();

    pub const fn new_empty() -> Self {
        Self {
            inum: 0,
            name: [0; DIRSIZE],
        }
    }

    fn from_bytes(bytes: &[u8; Self::SIZE]) -> Self {
        unsafe { ptr::read_unaligned(bytes.as_ptr() as *const Self) }
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self as *const Self as *const u8, Self::SIZE) }
    }

    fn from_inode(
        inode: &Inode,
        inner: &mut SleepLockGuard<'_, InodeInner>,
        offset: u32,
    ) -> Result<Self, KernelError> {
        let mut buf = [0; Self::SIZE];
        let read = inode.read(inner, offset, &mut buf)?;
        assert_eq!(read as usize, Self::SIZE, "dir read from inode");
        Ok(Self::from_bytes(&buf))
    }

    fn is_name_equal(&self, name: &str) -> bool {
        let end = self.name.iter().position(|&c| c == 0).unwrap_or(DIRSIZE);
        &self.name[..end] == name.as_bytes()
    }

    fn set_name(&mut self, name: &str) {
        self.name.fill(0);
        let bytes = name.as_bytes();
        let len = bytes.len().min(DIRSIZE);
        self.name[..len].copy_from_slice(&bytes[..len]);
    }

    /// Checks whether the directory is empty (only contains `.` and `..`).
    pub fn is_empty(inode: &Inode, inner: &mut SleepLockGuard<'_, InodeInner>) -> bool {
        for offset in ((2 * Self::SIZE as u32)..inner.size).step_by(Self::SIZE) {
            let dir = Self::from_inode(inode, inner, offset).unwrap();
            if dir.inum != 0 {
                return false;
            }
        }

        true
    }

    /// Looks up for a directory entry in a directory.
    /// If found, returns byte offset and Inode.
    pub fn lookup(
        inode: &Inode,
        inner: &mut SleepLockGuard<'_, InodeInner>,
        name: &str,
    ) -> Result<(u32, Inode), KernelError> {
        assert_eq!(inner.r#type, InodeType::Directory, "dirlookup not DIR");

        for offset in (0..inner.size).step_by(Self::SIZE) {
            let dir = Self::from_inode(inode, inner, offset)?;

            if dir.inum == 0 {
                continue;
            }

            if dir.is_name_equal(name) {
                // entry matches path element
                let dir_inode = Inode::get(inode.dev, dir.inum as u32)?;
                return Ok((offset, dir_inode));
            }
        }

        Err(KernelError::Fs)
    }

    /// Writes a new directory entry (name, inum) into the directory Inode.
    pub fn link(
        inode: &Inode,
        inner: &mut SleepLockGuard<'_, InodeInner>,
        name: &str,
        inum: u16,
    ) -> Result<(), KernelError> {
        // check the name is not present
        if let Ok((_, dir)) = Self::lookup(inode, inner, name) {
            dir.put();
            return Err(KernelError::Fs);
        }

        // look for an empty directory
        let mut dir = Self::new_empty();
        let mut offset = 0;

        while offset < inner.size {
            dir = Self::from_inode(inode, inner, offset)?;

            // if we find an empty slot break and use it.
            // otherwise, the dir will be appended to the end
            if dir.inum == 0 {
                break;
            } else {
                offset += Self::SIZE as u32;
            }
        }

        dir.set_name(name);
        dir.inum = inum;

        let write = inode.write(inner, offset, dir.as_bytes())?;
        if write as usize != Self::SIZE {
            return Err(KernelError::Fs);
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Path<'a>(&'a str);

impl<'a> Path<'a> {
    pub const fn new(name: &'a str) -> Path<'a> {
        Self(name)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn is_absolute(&self) -> bool {
        self.0.starts_with('/')
    }

    /// Returns (next path component, rest).
    /// The returned path has no leading slashes, so the caller can check to see if the component
    /// is the last one (Path == '\0').
    /// If no component to remove, returns None.
    pub fn next_component(&self) -> Option<(&'a str, Path<'a>)> {
        let s = self.0.trim_start_matches('/');

        if s.is_empty() {
            return None;
        }

        match s.find('/') {
            Some(i) => {
                let rest = s[i..].trim_start_matches('/');
                Some((&s[..i], Path(rest)))
            }
            None => Some((s, Path(""))),
        }
    }

    fn resolve_inner(&self, parent: bool) -> Result<(Inode, &'a str), KernelError> {
        let mut inode = if self.is_absolute() {
            Inode::get(ROOTDEV, ROOTINO)?
        } else {
            CPU_POOL.current_proc().unwrap().data().cwd.dup()
        };

        let mut name = "";
        let mut path = self.clone();

        // walk the path, one component at at time
        while let Some((component, rest)) = path.next_component() {
            let mut inner = inode.lock();

            if inner.r#type != InodeType::Directory {
                inode.unlock_put(inner);
                return Err(KernelError::Fs);
            }

            // stop one level early
            if parent && rest.is_empty() {
                inode.unlock(inner);
                return Ok((inode, component));
            }

            // get the next inode
            match Directory::lookup(&inode, &mut inner, component) {
                Ok((_, next)) => {
                    inode.unlock_put(inner);
                    inode = next;
                }
                Err(e) => {
                    inode.unlock_put(inner);
                    return Err(e);
                }
            }

            name = component;
            path = rest;
        }

        // we returned early to put the last inode
        if parent {
            inode.put();
            return Err(KernelError::Fs);
        }

        Ok((inode, name))
    }

    /// Resolves the full path to an inode.
    pub fn resolve(&self) -> Result<Inode, KernelError> {
        self.resolve_inner(false).map(|(inode, _)| inode)
    }

    /// Resolves to the parent directory, returning (parent, final_name).
    pub fn resolve_parent(&self) -> Result<(Inode, &'a str), KernelError> {
        self.resolve_inner(true)
    }
}
