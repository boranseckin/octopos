#![allow(unused)]

use crate::riscv::PGSIZE;

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

/// open sockets per system
pub const NSOCKET: usize = 16;

/// max exec arguments
pub const MAXARG: usize = 32;

/// max # of blocks any FS op writes
pub const MAXOPBLOCKS: usize = 10;

/// max data blocks in on-disk log
pub const LOGBLOCKS: usize = MAXOPBLOCKS * 3;

/// size of disk block cache
pub const NBUF: usize = MAXOPBLOCKS * 3;

/// maximum file path name
pub const MAXPATH: usize = 128;

/// maximum bytes for a proc name
pub const MAX_PROC_NAME: usize = 16;

/// size of the boot stack (debug needs more stack for error tracing)
#[cfg(debug_assertions)]
pub const BOOTSTACK_SIZE: usize = 4 * PGSIZE;
#[cfg(not(debug_assertions))]
pub const BOOTSTACK_SIZE: usize = PGSIZE;

/// kernel stack pages per process (debug needs more stack to dump)
#[cfg(debug_assertions)]
pub const NKSTACK_PAGES: usize = 8;
#[cfg(not(debug_assertions))]
pub const NKSTACK_PAGES: usize = 2;

/// user stack pages
pub const USERSTACK: usize = 4;

/// largest frame
/// a bigger one can jump over the guard page instead of faulting on it
pub const MAX_FRAME: usize = PGSIZE;

/// largest frame of a recursive function, since its cost multiplies with the depth
/// a depth limit of `N` costs at most `N x MAX_RECURSIVE_FRAME` bytes
pub const MAX_RECURSIVE_FRAME: usize = MAX_FRAME / 8;

/// largest stack buffer in user code
/// bigger ones go on the heap
pub const MAX_USER_STACK_BUFFER: usize = 512;

/// largest stack buffer in the kernel
/// release kernel stacks are one page and interrupts nest on them
pub const MAX_KERNEL_STACK_BUFFER: usize = 256;

/// timebase for qemu
pub const TIMEBASE_HZ: usize = 10_000_000;

/// ticks per second
pub const TICKS_PER_SEC: usize = 10;

/// print ignored errors
pub const TRACK_EXPECTED_ERRORS: bool = false;

/// print stack usage per proc on exit
pub const TRACK_STACK_USAGE: bool = false;
