use crate::proc::{self, CPU_POOL, Channel, PID};
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
    match proc::fork() {
        Ok(pid) => Ok(*pid),
        Err(_) => Err(SyscallError::ForkError),
    }
}

pub fn sys_wait(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let addr = args.get_addr(0);
    match proc::wait(addr) {
        Some(pid) => Ok(*pid),
        None => Err(SyscallError::WaitError),
    }
}

pub fn sys_sbrk(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let size = args.get_int(0);
    let addr = args.proc().data().size;

    match unsafe { proc::grow(size) } {
        Ok(_) => Ok(addr),
        Err(_) => Err(SyscallError::SbrkError),
    }
}

pub fn sys_sleep(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let duration = args.get_int(0).max(0) as usize;

    let mut ticks = TICKS_LOCK.lock();
    let ticks0 = ticks.clone();

    while *ticks - ticks0 < duration {
        if CPU_POOL.current_proc().unwrap().is_killed() {
            return Err(SyscallError::SleepError);
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
    let ticks = *TICKS_LOCK.lock();
    Ok(ticks)
}
