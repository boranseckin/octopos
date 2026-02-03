use core::fmt::Display;

use alloc::string::String;
use alloc::vec::Vec;

use crate::abi::Syscall;
use crate::file::File;
use crate::param::NOFILE;
use crate::proc::{Proc, TrapFrame, current_proc, current_proc_and_data_mut};
use crate::sysfile::*;
use crate::sysproc::*;
use crate::vm::VA;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallError {
    Unknown(usize),
    InvalidArgument(&'static str),
    FetchArgument,
    Proc(&'static str),
    File(&'static str),
    Console,
    Read,
    Write,
}

impl Display for SyscallError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SyscallError::Unknown(i) => write!(f, "unknown syscall {i}"),
            SyscallError::InvalidArgument(s) => write!(f, "invalid argument {s}"),
            SyscallError::FetchArgument => write!(f, "fetch argument"),
            SyscallError::Proc(s) => write!(f, "{s}"),
            SyscallError::File(s) => write!(f, "{s}"),
            SyscallError::Console => write!(f, "console error"),
            SyscallError::Read => write!(f, "read error"),
            SyscallError::Write => write!(f, "write error"),
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
    pub fn get_file(&self, index: usize) -> Result<(usize, File), SyscallError> {
        let fd: usize = try_log!(
            self.get_int(index)
                .try_into()
                .or(Err(SyscallError::InvalidArgument("fd conversion failed")))
        );

        if fd >= NOFILE {
            err!(SyscallError::InvalidArgument("fd out of range"));
        }

        if let Some(file) = &current_proc().data().open_files[fd] {
            return Ok((fd, file.clone()));
        }

        err!(SyscallError::InvalidArgument("fd not open"));
    }

    /// Fetches a null-terminated string from user space.
    pub fn fetch_string(&self, addr: VA, max: usize) -> Result<String, SyscallError> {
        let (_proc, data) = current_proc_and_data_mut();

        let mut result = String::with_capacity(max);

        let mut buf = [0u8; 1];
        for i in 0..max {
            try_log!(
                data.pagetable_mut()
                    .copy_from(VA::from(addr.as_usize() + i), &mut buf)
                    .inspect_err(|e| println!("copy_from failed: {:?}", e))
                    .map_err(|_| SyscallError::FetchArgument)
            );

            if buf[0] == 0 {
                return Ok(result);
            }

            result.push(buf[0] as char);
        }

        Ok(result)
    }

    /// Fetches a byte array from user space.
    pub fn fetch_bytes(&self, addr: VA, len: usize) -> Result<Vec<u8>, SyscallError> {
        let (_proc, data) = current_proc_and_data_mut();

        if addr >= data.size || addr + 64 > data.size {
            err!(SyscallError::FetchArgument);
        }

        let mut result = Vec::with_capacity(len);
        try_log!(
            data.pagetable_mut()
                .copy_from(addr, &mut result)
                .inspect_err(|e| println!("copy_from failed: {:?}", e))
                .map_err(|_| SyscallError::FetchArgument)
        );

        Ok(result)
    }
}

impl TryFrom<usize> for Syscall {
    type Error = SyscallError;

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
            _ => Err(SyscallError::Unknown(value)),
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

    #[cfg(debug_assertions)]
    println!(
        "syscall {} called from proc {} ({})",
        trapframe.a7,
        *proc.inner.lock().pid,
        proc.data().name,
    );

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

    trapframe.a0 = log!(result)
        .inspect_err(|e| {
            println!(
                "! syscall error ({}) from proc {} ({})",
                e,
                *proc.inner.lock().pid,
                proc.data().name,
            )
        })
        .unwrap_or(usize::MAX);

    #[cfg(debug_assertions)]
    println!("syscall {} -> {}", trapframe.a7, trapframe.a0);
}
