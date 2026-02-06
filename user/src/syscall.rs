pub mod raw {
    use core::arch::asm;

    use kernel::abi::{Stat, Syscall};

    #[inline(always)]
    fn syscall0(syscall: Syscall) -> usize {
        let ret: usize;
        unsafe {
            asm!(
                "ecall",
                in("a7") syscall as usize,
                lateout("a0") ret,
            );
        }
        ret
    }

    #[inline(always)]
    fn syscall1(syscall: Syscall, a0: usize) -> usize {
        let ret: usize;
        unsafe {
            asm!(
                "ecall",
                in("a7") syscall as usize,
                inlateout("a0") a0 => ret,
            );
        }
        ret
    }

    #[inline(always)]
    fn syscall2(syscall: Syscall, a0: usize, a1: usize) -> usize {
        let ret: usize;
        unsafe {
            asm!(
                "ecall",
                in("a7") syscall as usize,
                inlateout("a0") a0 => ret,
                in("a1") a1,
            );
        }
        ret
    }

    #[inline(always)]
    fn syscall3(syscall: Syscall, a0: usize, a1: usize, a2: usize) -> usize {
        let ret: usize;
        unsafe {
            asm!(
                "ecall",
                in("a7") syscall as usize,
                inlateout("a0") a0 => ret,
                in("a1") a1,
                in("a2") a2,
            );
        }
        ret
    }

    pub fn fork() -> usize {
        syscall0(Syscall::Fork)
    }

    pub fn exit(code: usize) -> ! {
        syscall1(Syscall::Exit, code);
        unreachable!();
    }

    pub fn wait(status: *mut usize) -> usize {
        syscall1(Syscall::Wait, status as usize)
    }

    pub fn pipe(fds: *mut usize) -> usize {
        syscall1(Syscall::Pipe, fds as usize)
    }

    pub fn read(fd: usize, buf: *mut u8, len: usize) -> usize {
        syscall3(Syscall::Read, fd, buf as usize, len)
    }

    pub fn write(fd: usize, buf: *const u8, len: usize) -> usize {
        syscall3(Syscall::Write, fd, buf as usize, len)
    }

    pub fn kill(pid: usize) -> usize {
        syscall1(Syscall::Kill, pid)
    }

    pub fn exec(path: *const u8, argv: *const *const u8) -> usize {
        syscall2(Syscall::Exec, path as usize, argv as usize)
    }

    pub fn fstat(fd: usize, stat: *mut Stat) -> usize {
        syscall2(Syscall::Fstat, fd, stat as usize)
    }

    pub fn chdir(path: *const u8) -> usize {
        syscall1(Syscall::Chdir, path as usize)
    }

    pub fn dup(fd: usize) -> usize {
        syscall1(Syscall::Dup, fd)
    }

    pub fn getpid() -> usize {
        syscall0(Syscall::Getpid)
    }

    pub fn sbrk(n: usize) -> usize {
        syscall1(Syscall::Sbrk, n)
    }

    pub fn sleep(ticks: usize) -> usize {
        syscall1(Syscall::Sleep, ticks)
    }

    pub fn uptime() -> usize {
        syscall0(Syscall::Uptime)
    }

    pub fn open(path: *const u8, flags: usize) -> usize {
        syscall2(Syscall::Open, path as usize, flags)
    }

    pub fn close(fd: usize) -> usize {
        syscall1(Syscall::Close, fd)
    }

    pub fn mknod(path: *const u8, major: usize, minor: usize) -> usize {
        syscall3(Syscall::Mknod, path as usize, major, minor)
    }

    pub fn unlink(path: *const u8) -> usize {
        syscall1(Syscall::Unlink, path as usize)
    }

    pub fn link(old: *const u8, new: *const u8) -> usize {
        syscall2(Syscall::Link, old as usize, new as usize)
    }

    pub fn mkdir(path: *const u8) -> usize {
        syscall1(Syscall::Mkdir, path as usize)
    }
}

use kernel::abi::{MAXPATH, Stat};

/// Error type for syscall wrappers.
///
/// The xv6 kernel only returns -1 (mapped to `usize::MAX`) on failure, so we cannot
/// distinguish specific kernel errors. Callers add their own context in messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysError {
    /// Path exceeds `MAXPATH` or contains embedded null bytes.
    InvalidPath,
    /// The kernel returned an error.
    Kernel,
}

impl core::fmt::Display for SysError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SysError::InvalidPath => write!(f, "invalid path"),
            SysError::Kernel => write!(f, "kernel error"),
        }
    }
}

/// A file descriptor returned by or passed to syscalls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fd(usize);

impl Fd {
    pub const STDIN: Fd = Fd(0);
    pub const STDOUT: Fd = Fd(1);
    pub const STDERR: Fd = Fd(2);

    /// Returns the raw file descriptor number.
    pub fn as_raw(&self) -> usize {
        self.0
    }
}

impl core::fmt::Display for Fd {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A validated path suitable for passing to syscalls.
///
/// Guarantees that the inner string is shorter than `MAXPATH` and contains no
/// embedded null bytes, so it can be safely null-terminated on the stack.
#[derive(Debug, Clone, Copy)]
struct Path<'a>(&'a str);

impl<'a> Path<'a> {
    fn new(s: &'a str) -> Result<Self, SysError> {
        if s.len() >= MAXPATH || s.bytes().any(|b| b == 0) {
            return Err(SysError::InvalidPath);
        }
        Ok(Self(s))
    }

    /// Creates a null-terminated C-string buffer on the stack.
    fn as_cpath(&self) -> [u8; MAXPATH] {
        let mut buf = [0u8; MAXPATH];
        buf[..self.0.len()].copy_from_slice(self.0.as_bytes());
        buf
    }
}

/// Converts a raw syscall return into `Result`, treating `usize::MAX` as error.
#[inline(always)]
fn check(ret: usize) -> Result<usize, SysError> {
    if ret == usize::MAX {
        Err(SysError::Kernel)
    } else {
        Ok(ret)
    }
}

/// Converts a raw syscall return into `Result<(), SysError>`.
#[inline(always)]
fn check_unit(ret: usize) -> Result<(), SysError> {
    check(ret).map(|_| ())
}

/// Validates a path string and creates a C-compatible path buffer.
fn validate_path(path: &str) -> Result<[u8; MAXPATH], SysError> {
    Ok(Path::new(path)?.as_cpath())
}

pub fn fork() -> Result<usize, SysError> {
    check(raw::fork())
}

pub fn exit(code: usize) -> ! {
    raw::exit(code)
}

pub fn exit_with_msg(msg: &str) -> ! {
    eprintln!("{}", msg);
    exit(1);
}

pub fn wait(status: &mut usize) -> Result<usize, SysError> {
    check(raw::wait(status as *mut usize))
}

pub fn pipe() -> Result<(Fd, Fd), SysError> {
    let mut fds = [0usize; 2];
    check_unit(raw::pipe(fds.as_mut_ptr()))?;
    Ok((Fd(fds[0]), Fd(fds[1])))
}

pub fn read(fd: Fd, buf: &mut [u8]) -> Result<usize, SysError> {
    check(raw::read(fd.as_raw(), buf.as_mut_ptr(), buf.len()))
}

pub fn write(fd: Fd, buf: &[u8]) -> Result<usize, SysError> {
    check(raw::write(fd.as_raw(), buf.as_ptr(), buf.len()))
}

pub fn kill(pid: usize) -> Result<(), SysError> {
    check_unit(raw::kill(pid))
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

    raw::exec(cpath.as_ptr(), ptrs.as_ptr());
    SysError::Kernel
}

pub fn fstat(fd: Fd, stat: &mut Stat) -> Result<(), SysError> {
    check_unit(raw::fstat(fd.as_raw(), stat as *mut Stat))
}

pub fn chdir(path: &str) -> Result<(), SysError> {
    let cpath = validate_path(path)?;
    check_unit(raw::chdir(cpath.as_ptr()))
}

pub fn dup(fd: Fd) -> Result<Fd, SysError> {
    check(raw::dup(fd.as_raw())).map(Fd)
}

pub fn getpid() -> usize {
    raw::getpid()
}

pub fn sbrk(n: isize) -> Result<usize, SysError> {
    check(raw::sbrk(n as usize))
}

pub fn sleep(ticks: usize) -> Result<(), SysError> {
    check_unit(raw::sleep(ticks))
}

pub fn uptime() -> usize {
    raw::uptime()
}

pub fn open(path: &str, flags: usize) -> Result<Fd, SysError> {
    let cpath = validate_path(path)?;
    check(raw::open(cpath.as_ptr(), flags)).map(Fd)
}

pub fn close(fd: Fd) -> Result<(), SysError> {
    check_unit(raw::close(fd.as_raw()))
}

pub fn mknod(path: &str, major: usize, minor: usize) -> Result<(), SysError> {
    let cpath = validate_path(path)?;
    check_unit(raw::mknod(cpath.as_ptr(), major, minor))
}

pub fn unlink(path: &str) -> Result<(), SysError> {
    let cpath = validate_path(path)?;
    check_unit(raw::unlink(cpath.as_ptr()))
}

pub fn link(old: &str, new: &str) -> Result<(), SysError> {
    let cold = validate_path(old)?;
    let cnew = validate_path(new)?;
    check_unit(raw::link(cold.as_ptr(), cnew.as_ptr()))
}

pub fn mkdir(path: &str) -> Result<(), SysError> {
    let cpath = validate_path(path)?;
    check_unit(raw::mkdir(cpath.as_ptr()))
}
