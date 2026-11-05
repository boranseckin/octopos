use core::sync::atomic::Ordering;

use crate::abi::ProcState;
use crate::memlayout::QEMU_POWER;
use crate::param::NPROC;
use crate::proc::{self, Channel, Pid, RawProc};
use crate::rng::rand_bytes;
use crate::syscall::{Result, SysError, SyscallArgs};
use crate::trap::TICKS;

#[repr(usize)]
pub enum ProcOp {
    Get = 1,
}

impl TryFrom<usize> for ProcOp {
    type Error = SysError;

    fn try_from(value: usize) -> core::result::Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Get),
            _ => Err(SysError::InvalidArgument),
        }
    }
}

/// Reads or modifies a process.
///
/// # Arguments
///
/// - `a0` (`ProcOp`): operation to perform.
/// - `a1` (`*mut RawProc`): pointer to the proc struct.
/// - `a2` (`usize`): operation-specific argument.
///
/// # Operations
///
/// - `Get`: writes the proc at index `a2` into `a1`. Indices start at 0 and are contiguous;
///   returns `NoEntry` when `a2` >= `NPROC`. The table may change between calls, so a listing can
///   skip or repeat an entry.
pub fn sys_proc(args: &SyscallArgs) -> Result<usize> {
    let op = ProcOp::try_from(args.get_int(0) as usize)?;
    let raw_proc_va = args.get_addr(1);

    match op {
        ProcOp::Get => {
            let index = args.get_int(2) as usize;

            if index >= NPROC {
                err!(SysError::NoEntry);
            }

            let proc = proc::PROC_TABLE.get(index);
            let mut raw_proc = RawProc::from(proc);

            if raw_proc.state == ProcState::Unused as u8 {
                raw_proc = RawProc::default();
            }

            proc::copy_struct_to_user(&raw_proc, raw_proc_va)?;
        }
    }

    Ok(0)
}

pub fn sys_exit(args: &SyscallArgs) -> ! {
    let n = args.get_int(0);
    proc::exit(n);
}

pub fn sys_getpid(args: &SyscallArgs) -> Result<usize> {
    let pid = args.proc().inner.lock().pid;
    Ok(*pid)
}

pub fn sys_fork(_args: &SyscallArgs) -> Result<usize> {
    Ok(*proc::fork()?)
}

pub fn sys_wait(args: &SyscallArgs) -> Result<usize> {
    let addr = args.get_addr(0);
    match proc::wait(addr) {
        Some(pid) => Ok(*pid),
        None => err!(SysError::NoChildren),
    }
}

pub fn sys_sbrk(args: &SyscallArgs) -> Result<usize> {
    let size = args.get_int(0);
    let addr = args.proc().size.load(Ordering::Relaxed);

    unsafe { proc::grow(size, size >= 0)? };
    Ok(addr)
}

pub fn sys_sleep(args: &SyscallArgs) -> Result<usize> {
    let duration = args.get_int(0).max(0) as usize;

    let mut ticks = TICKS.lock();
    let ticks0 = *ticks;

    while *ticks - ticks0 < duration {
        if proc::current_proc().is_killed() {
            err!(SysError::Interrupted);
        }

        ticks = proc::sleep(Channel::Ticks, ticks);
    }

    Ok(0)
}

pub fn sys_kill(args: &SyscallArgs) -> Result<usize> {
    let pid = args.get_int(0);

    // Safety: kernel will return an error if the process does not exist.
    if !proc::kill(unsafe { Pid::from_usize(pid as usize) }) {
        err!(SysError::NoProcess)
    }

    Ok(0)
}

pub fn sys_uptime(_args: &SyscallArgs) -> Result<usize> {
    let ticks = *TICKS.lock();
    Ok(ticks)
}

const RANDOM_CHUNK: usize = 256;

pub fn sys_random(args: &SyscallArgs) -> Result<usize> {
    let dest = args.get_addr(0);
    let len = args.get_int(1) as usize;

    // copy in chunks to avoid allocating memory
    let mut chunk = [0u8; RANDOM_CHUNK];

    for offset in (0..len).step_by(RANDOM_CHUNK) {
        let n = (len - offset).min(RANDOM_CHUNK);
        rand_bytes(&mut chunk[..n]);
        proc::copy_to_user(&chunk[..n], dest + offset)?;
    }

    Ok(0)
}

pub fn sys_poweroff(args: &SyscallArgs) -> ! {
    let code = match args.get_int(0) as u32 {
        0 => 0x5555,
        c => (c << 16) | 0x3333,
    };

    println!("! powering off...");

    unsafe { *(QEMU_POWER as *mut u32) = code };

    unreachable!("poweroff failed");
}

pub fn sys_reboot(_args: &SyscallArgs) -> ! {
    println!("! rebooting...");

    unsafe { *(QEMU_POWER as *mut u32) = 0x7777 };

    unreachable!("reboot failed");
}
