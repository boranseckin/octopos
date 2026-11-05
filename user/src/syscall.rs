use kernel::abi::{MAXPATH, SysError};

pub use kernel::abi::{InterfaceOp, RawInterface, RawRoute, RawSockAddr, RouteOp, Syscall};

/// A validated path suitable for passing to syscalls.
///
/// Guarantees that the inner string is shorter than `MAXPATH` and contains no
/// embedded null bytes, so it can be safely null-terminated on the stack.
#[derive(Debug, Clone, Copy)]
struct Path<'a>(&'a str);

impl<'a> Path<'a> {
    fn new(s: &'a str) -> Result<Self, SysError> {
        if s.len() >= MAXPATH || s.bytes().any(|b| b == 0) {
            return Err(SysError::NameTooLong);
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

/// Validates a path string and creates a C-compatible path buffer.
pub fn validate_path(path: &str) -> Result<[u8; MAXPATH], SysError> {
    Ok(Path::new(path)?.as_cpath())
}

/// Converts a raw signed syscall return into `Result`, treating negative values as error codes.
#[inline(always)]
pub fn check(ret: isize) -> Result<usize, SysError> {
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(SysError::from_code((-ret) as u16))
    }
}

/// Converts a raw syscall return into `Result<(), SysError>`.
#[inline(always)]
pub fn check_unit(ret: isize) -> Result<(), SysError> {
    check(ret).map(|_| ())
}

/// Unchecked syscalls using raw-pointers and lengths.
pub mod raw {
    use core::arch::asm;

    use kernel::abi::{
        InterfaceOp, ProcOp, RawInterface, RawProc, RawRoute, RawSockAddr, RouteOp, Stat, Syscall,
    };

    #[inline(always)]
    fn syscall0(syscall: Syscall) -> isize {
        let ret: isize;
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
    fn syscall1(syscall: Syscall, a0: usize) -> isize {
        let ret: isize;
        unsafe {
            asm!(
                "ecall",
                in("a7") syscall as usize,
                inlateout("a0") a0 as isize => ret,
            );
        }
        ret
    }

    #[inline(always)]
    fn syscall2(syscall: Syscall, a0: usize, a1: usize) -> isize {
        let ret: isize;
        unsafe {
            asm!(
                "ecall",
                in("a7") syscall as usize,
                inlateout("a0") a0 as isize => ret,
                in("a1") a1,
            );
        }
        ret
    }

    #[inline(always)]
    fn syscall3(syscall: Syscall, a0: usize, a1: usize, a2: usize) -> isize {
        let ret: isize;
        unsafe {
            asm!(
                "ecall",
                in("a7") syscall as usize,
                inlateout("a0") a0 as isize => ret,
                in("a1") a1,
                in("a2") a2,
            );
        }
        ret
    }

    #[inline(always)]
    fn syscall4(syscall: Syscall, a0: usize, a1: usize, a2: usize, a3: usize) -> isize {
        let ret: isize;
        unsafe {
            asm!(
                "ecall",
                in("a7") syscall as usize,
                inlateout("a0") a0 as isize => ret,
                in("a1") a1,
                in("a2") a2,
                in("a3") a3,
            );
        }
        ret
    }

    pub fn fork() -> isize {
        syscall0(Syscall::Fork)
    }

    pub fn exit(code: usize) -> ! {
        syscall1(Syscall::Exit, code);
        unreachable!();
    }

    pub fn wait(status: *mut usize) -> isize {
        syscall1(Syscall::Wait, status as usize)
    }

    pub fn pipe(fds: *mut usize) -> isize {
        syscall1(Syscall::Pipe, fds as usize)
    }

    pub fn read(fd: usize, buf: *mut u8, len: usize) -> isize {
        syscall3(Syscall::Read, fd, buf as usize, len)
    }

    pub fn write(fd: usize, buf: *const u8, len: usize) -> isize {
        syscall3(Syscall::Write, fd, buf as usize, len)
    }

    pub fn proc_get(proc: *mut RawProc, index: usize) -> isize {
        syscall3(Syscall::Proc, ProcOp::Get as usize, proc as usize, index)
    }

    pub fn kill(pid: usize) -> isize {
        syscall1(Syscall::Kill, pid)
    }

    pub fn exec(path: *const u8, argv: *const *const u8) -> isize {
        syscall2(Syscall::Exec, path as usize, argv as usize)
    }

    pub fn fstat(fd: usize, stat: *mut Stat) -> isize {
        syscall2(Syscall::Fstat, fd, stat as usize)
    }

    pub fn chdir(path: *const u8) -> isize {
        syscall1(Syscall::Chdir, path as usize)
    }

    pub fn dup(fd: usize) -> isize {
        syscall1(Syscall::Dup, fd)
    }

    pub fn getpid() -> isize {
        syscall0(Syscall::Getpid)
    }

    pub fn sbrk(n: usize) -> isize {
        syscall1(Syscall::Sbrk, n)
    }

    pub fn sleep(ticks: usize) -> isize {
        syscall1(Syscall::Sleep, ticks)
    }

    pub fn uptime() -> isize {
        syscall0(Syscall::Uptime)
    }

    pub fn open(path: *const u8, flags: usize) -> isize {
        syscall2(Syscall::Open, path as usize, flags)
    }

    pub fn close(fd: usize) -> isize {
        syscall1(Syscall::Close, fd)
    }

    pub fn mknod(path: *const u8, major: usize, minor: usize) -> isize {
        syscall3(Syscall::Mknod, path as usize, major, minor)
    }

    pub fn unlink(path: *const u8) -> isize {
        syscall1(Syscall::Unlink, path as usize)
    }

    pub fn link(old: *const u8, new: *const u8) -> isize {
        syscall2(Syscall::Link, old as usize, new as usize)
    }

    pub fn mkdir(path: *const u8) -> isize {
        syscall1(Syscall::Mkdir, path as usize)
    }

    pub fn poweroff(code: u32) -> ! {
        syscall1(Syscall::Poweroff, code as usize);
        unreachable!();
    }

    pub fn reboot() -> ! {
        syscall0(Syscall::Reboot);
        unreachable!();
    }

    pub fn ioctl(fd: usize, cmd: usize, arg: usize) -> isize {
        syscall3(Syscall::Ioctl, fd, cmd, arg)
    }

    pub fn socket(typ: usize, addr: *const RawSockAddr) -> isize {
        syscall2(Syscall::Socket, typ, addr as usize)
    }

    pub fn send(fd: usize, buf: *const u8, len: usize, dest_addr: *const RawSockAddr) -> isize {
        syscall4(Syscall::Send, fd, buf as usize, len, dest_addr as usize)
    }

    pub fn receive(fd: usize, buf: *mut u8, len: usize, src_addr: *mut RawSockAddr) -> isize {
        syscall4(Syscall::Receive, fd, buf as usize, len, src_addr as usize)
    }

    pub fn interface_get(interface: *mut RawInterface, index: usize) -> isize {
        syscall3(
            Syscall::Interface,
            InterfaceOp::Get as usize,
            interface as usize,
            index,
        )
    }

    pub fn interface_set_ipv4(interface: *const RawInterface) -> isize {
        syscall3(
            Syscall::Interface,
            InterfaceOp::SetIpv4 as usize,
            interface as usize,
            0,
        )
    }

    pub fn route_get(route: *mut RawRoute, index: usize) -> isize {
        syscall3(Syscall::Route, RouteOp::Get as usize, route as usize, index)
    }

    pub fn route_add(route: *const RawRoute, flags: usize) -> isize {
        syscall3(Syscall::Route, RouteOp::Add as usize, route as usize, flags)
    }

    pub fn route_remove(route: *const RawRoute) -> isize {
        syscall2(Syscall::Route, RouteOp::Remove as usize, route as usize)
    }

    pub fn random(buf: *mut u8, len: usize) -> isize {
        syscall2(Syscall::Random, buf as usize, len)
    }
}
