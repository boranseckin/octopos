/// maximum number of CPUs
pub const NCPU: usize = 8;
/// maximum number of processes
pub const NPROC: usize = 64;
/// open files per process
pub const NOFILE: usize = 16;
/// open files per system
pub const NFILE: usize = 100;
/// maximum number of active inodes
pub const NINODE: usize = 50;
/// maximum major device number
pub const NDEV: usize = 10;
/// device nubmer of file system root disk
pub const ROOTDEV: u32 = 1;
/// max exec arguments
pub const MAXARG: usize = 32;
/// max # of blocks any FS op writes
pub const MAXOPBLOCKS: usize = 10;
/// max data blocks in on-disk log
pub const LOGBLOCKS: usize = MAXOPBLOCKS * 3;
/// size of disk block cache
pub const NBUF: usize = MAXOPBLOCKS * 3;
/// size of file system in blocks
pub const FSSIZE: usize = 2000;
/// maximum file path name
pub const MAXPATH: usize = 128;
/// kernel stack pages per process (debug needs more stack to dump)
#[cfg(debug_assertions)]
pub const NKSTACK_PAGES: usize = 8;
#[cfg(not(debug_assertions))]
pub const NKSTACK_PAGES: usize = 1;
/// user stack pages
pub const USERSTACK: usize = 1;
