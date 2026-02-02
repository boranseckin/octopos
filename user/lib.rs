#![no_std]

use core::arch::asm;
use core::panic::PanicInfo;

use kernel::abi::Syscall;

pub const O_RDONLY: usize = 0x000;
pub const O_WRONLY: usize = 0x001;
pub const O_RDWR: usize = 0x002;
pub const O_CREATE: usize = 0x200;
pub const O_TRUNC: usize = 0x400;

pub const CONSOLE: usize = 1;

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

pub fn wait(status: &mut usize) -> usize {
    syscall1(Syscall::Wait, status as *mut usize as usize)
}

pub fn pipe(fds: &mut [usize; 2]) -> usize {
    syscall1(Syscall::Pipe, fds.as_mut_ptr() as usize)
}

pub fn read(fd: usize, buf: &mut [u8]) -> usize {
    syscall3(Syscall::Read, fd, buf.as_mut_ptr() as usize, buf.len())
}

pub fn kill(pid: usize) -> usize {
    syscall1(Syscall::Kill, pid)
}

pub fn exec(path: &[u8], argv: &[*const u8]) -> usize {
    syscall2(
        Syscall::Exec,
        path.as_ptr() as usize,
        argv.as_ptr() as usize,
    )
}

pub fn fstat(fd: usize, stat: *mut u8) -> usize {
    syscall2(Syscall::Fstat, fd, stat as usize)
}

pub fn chdir(path: &[u8]) -> usize {
    syscall1(Syscall::Chdir, path.as_ptr() as usize)
}

pub fn dup(fd: usize) -> usize {
    syscall1(Syscall::Dup, fd)
}

pub fn getpid() -> usize {
    syscall0(Syscall::Getpid)
}

pub fn sbrk(n: isize) -> usize {
    syscall1(Syscall::Sbrk, n as usize)
}

pub fn sleep(ticks: usize) -> usize {
    syscall1(Syscall::Sleep, ticks)
}

pub fn uptime() -> usize {
    syscall0(Syscall::Uptime)
}

pub fn open(path: &[u8], flags: usize) -> usize {
    syscall2(Syscall::Open, path.as_ptr() as usize, flags)
}

pub fn write(fd: usize, buf: &[u8]) -> usize {
    syscall3(Syscall::Write, fd, buf.as_ptr() as usize, buf.len())
}

pub fn mknod(path: &[u8], major: usize, minor: usize) -> usize {
    syscall3(Syscall::Mknod, path.as_ptr() as usize, major, minor)
}

pub fn unlink(path: &[u8]) -> usize {
    syscall1(Syscall::Unlink, path.as_ptr() as usize)
}

pub fn link(old: &[u8], new: &[u8]) -> usize {
    syscall2(Syscall::Link, old.as_ptr() as usize, new.as_ptr() as usize)
}

pub fn mkdir(path: &[u8]) -> usize {
    syscall1(Syscall::Mkdir, path.as_ptr() as usize)
}

pub fn close(fd: usize) -> usize {
    syscall1(Syscall::Close, fd)
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(1)
}
