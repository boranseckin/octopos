use alloc::string::String;
use alloc::vec::Vec;

use crate::println;
use crate::proc::{CPU_POOL, Proc, TrapFrame};
use crate::sysfile::*;
use crate::sysproc::*;
use crate::vm::VA;

pub enum SyscallError {
    Unknown(usize),
    ForkError,
    WaitError,
    SbrkError,
    SleepError,
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

    /// Fetches a null-terminated string from user space.
    pub fn get_string(&self, index: usize, max: usize) -> Result<String, SyscallError> {
        unimplemented!()
    }

    /// Fetches a byte array from user space.
    pub fn get_bytes(&self, index: usize, len: usize) -> Result<Vec<u8>, SyscallError> {
        unimplemented!()
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
pub unsafe fn syscall(trapframe: &mut TrapFrame) {
    let proc = CPU_POOL.current_proc().unwrap();
    let args = SyscallArgs::new(trapframe, proc);

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
        Err(e) => {
            println!(
                "{} {}: unknown syscall {}",
                *proc.inner.lock().pid,
                proc.data().name,
                trapframe.a7
            );
            Err(e)
        }
    };

    trapframe.a0 = result.unwrap_or(usize::MAX);
}
