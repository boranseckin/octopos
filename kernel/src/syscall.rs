use core::fmt::Display;

use alloc::string::String;

use crate::file::File;
use crate::fs::FsError;
use crate::param::NOFILE;
use crate::proc::{Proc, TrapFrame, current_proc, current_proc_and_data_mut};
use crate::sysfile::*;
use crate::sysproc::*;
use crate::vm::VA;

/// Syscall error codes using POSIX-standard numeric values.
///
/// Kernel encodes `-(error_code as isize)` in the return register (`a0`).
/// User space decodes negative values back into `SysError` variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum SysError {
    NotPermitted = 1,
    NoEntry = 2,
    NoProcess = 3,
    Interrupted = 4,
    IoError = 5,
    InvalidExecutable = 8,
    BadDescriptor = 9,
    NoChildren = 10,
    ResourceUnavailable = 11,
    OutOfMemory = 12,
    BadAddress = 14,
    AlreadyExists = 17,
    CrossDeviceLink = 18,
    NotDirectory = 20,
    IsDirectory = 21,
    InvalidArgument = 22,
    FileTableFull = 23,
    TooManyFiles = 24,
    NoSpace = 28,
    TooManyLinks = 31,
    BrokenPipe = 32,
    NameTooLong = 36,
    NotImplemented = 38,
    NotEmpty = 39,
}

impl SysError {
    /// Returns the error code for this error.
    pub fn as_code(self) -> u16 {
        self as u16
    }

    /// Decodes an error code into a `SysError` variant.
    pub fn from_code(code: u16) -> Self {
        match code {
            1 => Self::NotPermitted,
            2 => Self::NoEntry,
            3 => Self::NoProcess,
            4 => Self::Interrupted,
            5 => Self::IoError,
            8 => Self::InvalidExecutable,
            9 => Self::BadDescriptor,
            10 => Self::NoChildren,
            11 => Self::ResourceUnavailable,
            12 => Self::OutOfMemory,
            14 => Self::BadAddress,
            17 => Self::AlreadyExists,
            18 => Self::CrossDeviceLink,
            20 => Self::NotDirectory,
            21 => Self::IsDirectory,
            22 => Self::InvalidArgument,
            23 => Self::FileTableFull,
            24 => Self::TooManyFiles,
            28 => Self::NoSpace,
            31 => Self::TooManyLinks,
            32 => Self::BrokenPipe,
            36 => Self::NameTooLong,
            38 => Self::NotImplemented,
            39 => Self::NotEmpty,
            _ => Self::InvalidArgument,
        }
    }
}

impl Display for SysError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SysError::NotPermitted => write!(f, "operation not permitted"),
            SysError::NoEntry => write!(f, "no such file or directory"),
            SysError::NoProcess => write!(f, "no such process"),
            SysError::Interrupted => write!(f, "interrupted"),
            SysError::IoError => write!(f, "input/output error"),
            SysError::InvalidExecutable => write!(f, "exec format error"),
            SysError::BadDescriptor => write!(f, "bad file descriptor"),
            SysError::NoChildren => write!(f, "no child processes"),
            SysError::ResourceUnavailable => write!(f, "resource temporarily unavailable"),
            SysError::OutOfMemory => write!(f, "cannot allocate memory"),
            SysError::BadAddress => write!(f, "bad address"),
            SysError::AlreadyExists => write!(f, "file exists"),
            SysError::CrossDeviceLink => write!(f, "cross-device link"),
            SysError::NotDirectory => write!(f, "not a directory"),
            SysError::IsDirectory => write!(f, "is a directory"),
            SysError::InvalidArgument => write!(f, "invalid argument"),
            SysError::FileTableFull => write!(f, "too many open files in system"),
            SysError::TooManyFiles => write!(f, "too many open files"),
            SysError::NoSpace => write!(f, "no space left on device"),
            SysError::TooManyLinks => write!(f, "too many links"),
            SysError::BrokenPipe => write!(f, "broken pipe"),
            SysError::NameTooLong => write!(f, "file name too long"),
            SysError::NotImplemented => write!(f, "function not implemented"),
            SysError::NotEmpty => write!(f, "directory not empty"),
        }
    }
}

impl From<FsError> for SysError {
    fn from(e: FsError) -> Self {
        match e {
            FsError::OutOfBlock | FsError::OutOfInode => SysError::NoSpace,
            FsError::OutOfFile | FsError::OutOfPipe => SysError::FileTableFull,
            FsError::OutOfRange => SysError::InvalidArgument,
            FsError::Read | FsError::Write => SysError::IoError,
            FsError::Create => SysError::NoSpace,
            FsError::Link => SysError::AlreadyExists,
            FsError::Resolve => SysError::NoEntry,
            FsError::Type => SysError::InvalidArgument,
            FsError::Copy => SysError::BadAddress,
        }
    }
}

/// Wrapper for extracting typed syscall arguments from trapframe.
pub struct SyscallArgs<'a> {
    trapframe: &'a TrapFrame,
    proc: &'static Proc,
}

impl<'a> SyscallArgs<'a> {
    /// Creates a new SyscallArgs
    fn new(trapframe: &'a TrapFrame, proc: &'static Proc) -> Self {
        Self { trapframe, proc }
    }

    pub fn proc(&self) -> &Proc {
        self.proc
    }

    /// Returns the argument at the given index as a usize.
    pub fn get_raw(&self, index: usize) -> usize {
        match index {
            0 => self.trapframe.a0,
            1 => self.trapframe.a1,
            2 => self.trapframe.a2,
            3 => self.trapframe.a3,
            4 => self.trapframe.a4,
            5 => self.trapframe.a5,
            _ => panic!("invalid syscall argument index {}", index),
        }
    }

    /// Returns the argument at the given index as an isize.
    pub fn get_int(&self, index: usize) -> isize {
        self.get_raw(index) as isize
    }

    /// Returns the argument at the given index as a virtual address.
    ///
    /// Does not check for legality, since `copyin`/`copyout` will do that.
    pub fn get_addr(&self, index: usize) -> VA {
        VA::from(self.get_raw(index))
    }

    /// Fetch the nth word-sized system call argument as a file descriptor and return both the
    /// descriptor and the corresponding `File`.
    pub fn get_file(&self, index: usize) -> Result<(usize, File), SysError> {
        let fd: usize = try_log!(
            self.get_int(index)
                .try_into()
                .or(Err(SysError::BadDescriptor))
        );

        if fd >= NOFILE {
            err!(SysError::BadDescriptor);
        }

        if let Some(file) = &current_proc().data().open_files[fd] {
            return Ok((fd, file.clone()));
        }

        err!(SysError::BadDescriptor);
    }

    /// Fetches a null-terminated string from user space.
    pub fn fetch_string(&self, addr: VA, max: usize) -> Result<String, SysError> {
        let (_proc, data) = current_proc_and_data_mut();

        let mut result = String::with_capacity(max);

        let mut buf = [0u8; 1];
        for i in 0..max {
            try_log!(
                data.pagetable_mut()
                    .copy_from(VA::from(addr.as_usize() + i), &mut buf)
                    .map_err(|_| SysError::BadAddress)
            );

            if buf[0] == 0 {
                return Ok(result);
            }

            result.push(buf[0] as char);
        }

        Ok(result)
    }
}

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

impl TryFrom<usize> for Syscall {
    type Error = SysError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Syscall::Fork),
            2 => Ok(Syscall::Exit),
            3 => Ok(Syscall::Wait),
            4 => Ok(Syscall::Pipe),
            5 => Ok(Syscall::Read),
            6 => Ok(Syscall::Kill),
            7 => Ok(Syscall::Exec),
            8 => Ok(Syscall::Fstat),
            9 => Ok(Syscall::Chdir),
            10 => Ok(Syscall::Dup),
            11 => Ok(Syscall::Getpid),
            12 => Ok(Syscall::Sbrk),
            13 => Ok(Syscall::Sleep),
            14 => Ok(Syscall::Uptime),
            15 => Ok(Syscall::Open),
            16 => Ok(Syscall::Write),
            17 => Ok(Syscall::Mknod),
            18 => Ok(Syscall::Unlink),
            19 => Ok(Syscall::Link),
            20 => Ok(Syscall::Mkdir),
            21 => Ok(Syscall::Close),
            _ => Err(SysError::NotImplemented),
        }
    }
}

/// Handle a system call.
///
/// # Safety
/// Called from `usertrap` in `trap.rs`.
#[unsafe(no_mangle)]
pub unsafe fn syscall(trapframe: &mut TrapFrame) {
    let proc = current_proc();
    let args = SyscallArgs::new(trapframe, proc);

    // #[cfg(debug_assertions)]
    // println!(
    //     "syscall {} called from proc {} ({})",
    //     trapframe.a7,
    //     *proc.inner.lock().pid,
    //     proc.data().name,
    // );

    let result = match Syscall::try_from(trapframe.a7) {
        Ok(syscall) => match syscall {
            Syscall::Fork => sys_fork(&args),
            Syscall::Exit => sys_exit(&args),
            Syscall::Wait => sys_wait(&args),
            Syscall::Pipe => sys_pipe(&args),
            Syscall::Read => sys_read(&args),
            Syscall::Kill => sys_kill(&args),
            Syscall::Exec => sys_exec(&args),
            Syscall::Fstat => sys_fstat(&args),
            Syscall::Chdir => sys_chdir(&args),
            Syscall::Dup => sys_dup(&args),
            Syscall::Getpid => sys_getpid(&args),
            Syscall::Sbrk => sys_sbrk(&args),
            Syscall::Sleep => sys_sleep(&args),
            Syscall::Uptime => sys_uptime(&args),
            Syscall::Open => sys_open(&args),
            Syscall::Write => sys_write(&args),
            Syscall::Mknod => sys_mknod(&args),
            Syscall::Unlink => sys_unlink(&args),
            Syscall::Link => sys_link(&args),
            Syscall::Mkdir => sys_mkdir(&args),
            Syscall::Close => sys_close(&args),
        },
        Err(e) => Err(e),
    };

    trapframe.a0 = match log!(result) {
        Ok(v) => v,
        Err(error) => {
            #[cfg(debug_assertions)]
            println!(
                "! syscall error ({}) from proc {} ({})",
                error,
                *proc.inner.lock().pid,
                proc.data().name,
            );
            (-(error.as_code() as isize)) as usize
        }
    };

    // #[cfg(debug_assertions)]
    // println!("syscall {} -> {}", trapframe.a7, trapframe.a0);
}
