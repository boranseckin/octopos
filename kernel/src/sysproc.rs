use crate::proc::{self, Channel, Pid, current_proc};
use crate::syscall::{SysError, SyscallArgs};
use crate::trap::TICKS;

pub fn sys_exit(args: &SyscallArgs) -> ! {
    let n = args.get_int(0);
    proc::exit(n);
}

pub fn sys_getpid(args: &SyscallArgs) -> Result<usize, SysError> {
    let pid = args.proc().inner.lock().pid;
    Ok(*pid)
}

pub fn sys_fork(_args: &SyscallArgs) -> Result<usize, SysError> {
    match log!(proc::fork()) {
        Ok(pid) => Ok(*pid),
        Err(_) => Err(SysError::ResourceUnavailable),
    }
}

pub fn sys_wait(args: &SyscallArgs) -> Result<usize, SysError> {
    let addr = args.get_addr(0);
    match proc::wait(addr) {
        Some(pid) => Ok(*pid),
        None => err!(SysError::NoChildren),
    }
}

pub fn sys_sbrk(args: &SyscallArgs) -> Result<usize, SysError> {
    let size = args.get_int(0);
    let addr = args.proc().data().size;

    match unsafe { log!(proc::grow(size)) } {
        Ok(_) => Ok(addr),
        Err(_) => Err(SysError::OutOfMemory),
    }
}

pub fn sys_sleep(args: &SyscallArgs) -> Result<usize, SysError> {
    let duration = args.get_int(0).max(0) as usize;

    let mut ticks = TICKS.lock();
    let ticks0 = *ticks;

    while *ticks - ticks0 < duration {
        if current_proc().is_killed() {
            return Err(SysError::Interrupted);
        }

        ticks = proc::sleep(Channel::Ticks, ticks);
    }

    Ok(0)
}

pub fn sys_kill(args: &SyscallArgs) -> Result<usize, SysError> {
    let pid = args.get_int(0);

    // Safety: kernel will return an error if the process does not exist.
    if proc::kill(unsafe { Pid::from_usize(pid as usize) }) {
        Ok(0)
    } else {
        Err(SysError::NoProcess)
    }
}

pub fn sys_uptime(_args: &SyscallArgs) -> Result<usize, SysError> {
    let ticks = *TICKS.lock();
    Ok(ticks)
}
