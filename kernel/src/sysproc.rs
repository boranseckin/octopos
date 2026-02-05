use crate::proc::{self, Channel, PID, current_proc};
use crate::syscall::{SyscallArgs, SyscallError};
use crate::trap::TICKS;

pub fn sys_exit(args: &SyscallArgs) -> ! {
    let n = args.get_int(0);
    proc::exit(n);
}

pub fn sys_getpid(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let pid = args.proc().inner.lock().pid;
    Ok(*pid)
}

pub fn sys_fork(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    match log!(proc::fork()) {
        Ok(pid) => Ok(*pid),
        Err(_) => Err(SyscallError::Proc("sys_fork")),
    }
}

pub fn sys_wait(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let addr = args.get_addr(0);
    match proc::wait(addr) {
        Some(pid) => Ok(*pid),
        None => err!(SyscallError::Proc("sys_wait")),
    }
}

pub fn sys_sbrk(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let size = args.get_int(0);
    let addr = args.proc().data().size;

    match unsafe { log!(proc::grow(size)) } {
        Ok(_) => Ok(addr),
        Err(_) => Err(SyscallError::Proc("sys_sbrk")),
    }
}

pub fn sys_sleep(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let duration = args.get_int(0).max(0) as usize;

    let mut ticks = TICKS.lock();
    let ticks0 = *ticks;

    while *ticks - ticks0 < duration {
        if current_proc().is_killed() {
            return Err(SyscallError::Proc("sys_sleep"));
        }

        ticks = proc::sleep(Channel::Ticks, ticks);
    }

    Ok(0)
}

pub fn sys_kill(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let pid = args.get_int(0);

    Ok(proc::kill(PID::from(pid as usize)).into())
}

pub fn sys_uptime(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    let ticks = *TICKS.lock();
    Ok(ticks)
}
