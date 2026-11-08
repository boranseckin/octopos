use core::fmt::Display;
use core::iter;
use core::ptr;

use alloc::vec::Vec;

use kernel::abi::SysError;

use crate::syscall::{check, check_unit, raw, validate_path};

pub use kernel::abi::{MAXARG, ProcName, ProcOp, ProcState, RawProc, TICKS_PER_SEC};

#[derive(Debug, Clone, Copy)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub state: ProcState,
    pub name: ProcName,
    pub size: usize,
    pub killed: bool,
    pub kernel: bool,
}

impl TryFrom<RawProc> for ProcInfo {
    type Error = SysError;

    fn try_from(value: RawProc) -> Result<Self, Self::Error> {
        Ok(Self {
            pid: value.pid,
            ppid: value.ppid,
            state: ProcState::try_from(value.state)?,
            name: value.name,
            size: value.size as usize,
            killed: value.killed != 0,
            kernel: value.kernel != 0,
        })
    }
}

pub fn fork() -> Result<usize, SysError> {
    check(raw::fork())
}

/// Replaces the current process image with the program at `path`.
///
/// `argv` contains the argument strings. This function packs them into a contiguous
/// stack buffer with null terminators and builds the pointer array expected by the kernel.
///
/// Returns `SysError` because if `exec` returns at all, it failed.
pub fn exec(path: &str, argv: &[&str]) -> SysError {
    let cpath = match validate_path(path) {
        Ok(cpath) => cpath,
        Err(e) => return e,
    };

    if argv.len() > MAXARG {
        return SysError::InvalidArgument;
    }

    let total = argv.iter().map(|a| a.len() + 1).sum();
    let mut buf = Vec::with_capacity(total);
    let mut offsets = Vec::with_capacity(argv.len());

    for arg in argv.iter() {
        // record when this arg started
        offsets.push(buf.len());
        // push the arg and a null byte after
        buf.extend_from_slice(arg.as_bytes());
        buf.push(0);
    }

    // turn offsets into pointers and append one null byte
    let ptrs: Vec<_> = offsets
        .iter()
        .map(|&o| buf[o..].as_ptr())
        .chain(iter::once(ptr::null()))
        .collect();

    let ret = raw::exec(cpath.as_ptr(), ptrs.as_ptr());
    // exec only returns on failure
    SysError::from_code((-ret) as u16)
}

pub fn exit(code: usize) -> ! {
    raw::exit(code)
}

pub fn exit_with_msg(msg: impl Display) -> ! {
    eprintln!("{}", msg);
    exit(1);
}

pub fn wait(status: &mut usize) -> Result<usize, SysError> {
    check(raw::wait(status as *mut usize))
}

pub fn kill(pid: usize) -> Result<(), SysError> {
    check_unit(raw::kill(pid))
}

pub fn getpid() -> usize {
    raw::getpid() as usize
}

pub fn sbrk(n: isize) -> Result<usize, SysError> {
    check(raw::sbrk(n as usize))
}

pub fn sleep(ticks: usize) -> Result<(), SysError> {
    check_unit(raw::sleep(ticks))
}

pub fn proc_get(index: usize) -> Result<Option<ProcInfo>, SysError> {
    let mut raw_proc = RawProc::default();

    check(raw::proc_get(&mut raw_proc as *mut RawProc, index))?;

    if raw_proc.state == ProcState::Unused as u8 {
        return Ok(None);
    }

    Ok(Some(ProcInfo::try_from(raw_proc)?))
}

pub fn uptime() -> usize {
    raw::uptime() as usize
}

pub fn poweroff(code: u32) -> ! {
    raw::poweroff(code)
}

pub fn reboot() -> ! {
    raw::reboot()
}
