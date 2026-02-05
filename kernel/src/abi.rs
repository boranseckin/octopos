/// System call numbers
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syscall {
    Fork = 1,
    Exit = 2,
    Wait = 3,
    Pipe = 4,
    Read = 5,
    Kill = 6,
    Exec = 7,
    Fstat = 8,
    Chdir = 9,
    Dup = 10,
    Getpid = 11,
    Sbrk = 12,
    Sleep = 13,
    Uptime = 14,
    Open = 15,
    Write = 16,
    Mknod = 17,
    Unlink = 18,
    Link = 19,
    Mkdir = 20,
    Close = 21,
}

/// File open flags
pub struct OpenFlag;

impl OpenFlag {
    pub const READ_ONLY: usize = 0x000;
    pub const WRITE_ONLY: usize = 0x001;
    pub const READ_WRITE: usize = 0x002;
    pub const CREATE: usize = 0x200;
    pub const TRUNCATE: usize = 0x400;
}

/// Console device major number
pub const CONSOLE: usize = 1;
