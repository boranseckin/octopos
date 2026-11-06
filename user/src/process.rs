use core::fmt::Display;

use kernel::abi::SysError;

use crate::syscall::{check, check_unit, raw, validate_path};

pub use kernel::abi::{ProcName, ProcOp, ProcState, RawProc, TICKS_PER_SEC};

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

    const MAX_ARGV: usize = 16;
    const BUF_SIZE: usize = 512;

    let mut buf = [0u8; BUF_SIZE];
    let mut ptrs: [*const u8; MAX_ARGV + 1] = [core::ptr::null(); MAX_ARGV + 1];
    let mut offset = 0;

    for (i, arg) in argv.iter().enumerate().take(MAX_ARGV) {
        ptrs[i] = buf[offset..].as_ptr();
        buf[offset..offset + arg.len()].copy_from_slice(arg.as_bytes());
        // buf is zeroed, so the byte after the arg is already a null terminator
        offset += arg.len() + 1;
    }
    // ptrs is already null-terminated (initialized to null)

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
