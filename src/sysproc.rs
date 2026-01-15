use crate::proc::{self, PID};
use crate::syscall::{SyscallArgs, SyscallError};
use crate::trap::TICKS_LOCK;

pub fn sys_exit(args: &SyscallArgs) -> ! {
    let n = args.get_int(0);
    proc::exit(n);
}

pub fn sys_getpid(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let pid = args.proc().inner.lock().pid;
    Ok(*pid)
}

pub fn sys_fork(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    Ok(*proc::fork())
}

pub fn sys_wait(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let addr = args.get_addr(0);
    let pid = proc::wait(addr).unwrap_or(PID::from(usize::MAX));
    Ok(*pid)
}

pub fn sys_sbrk(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_sleep(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_kill(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_uptime(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    let ticks = *TICKS_LOCK.lock();
    Ok(ticks)
}
