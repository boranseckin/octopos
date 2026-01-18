/// maximum number of CPUs
pub const NCPU: usize = 8;
/// maximum number of processes
pub const NPROC: usize = 64;
/// max # of blocks any FS op writes
pub const MAXOPBLOCKS: usize = 10;
/// max data blocks in on-disk log
pub const LOGSIZE: usize = MAXOPBLOCKS * 3;
/// size of disk block cache
pub const NBUF: usize = MAXOPBLOCKS * 3;
