#![no_std]

use core::arch::asm;
use core::panic::PanicInfo;

use kernel::syscall::Syscall;

const SYS_FORK: usize = 1;
const SYS_EXIT: usize = 2;
const SYS_WAIT: usize = 3;
const SYS_PIPE: usize = 4;
const SYS_READ: usize = 5;
const SYS_KILL: usize = 6;
const SYS_EXEC: usize = 7;
const SYS_FSTAT: usize = 8;
const SYS_CHDIR: usize = 9;
const SYS_DUP: usize = 10;
const SYS_GETPID: usize = 11;
const SYS_SBRK: usize = 12;
const SYS_SLEEP: usize = 13;
const SYS_UPTIME: usize = 14;
const SYS_OPEN: usize = 15;
const SYS_WRITE: usize = 16;
const SYS_MKNOD: usize = 17;
const SYS_UNLINK: usize = 18;
const SYS_LINK: usize = 19;
const SYS_MKDIR: usize = 20;
const SYS_CLOSE: usize = 21;

pub const O_RDONLY: usize = 0x000;
pub const O_WRONLY: usize = 0x001;
pub const O_RDWR: usize = 0x002;
pub const O_CREATE: usize = 0x200;
pub const O_TRUNC: usize = 0x400;

pub const CONSOLE: usize = 1;

#[inline(always)]
fn syscall0(num: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!(
            "ecall",
            in("a7") num,
            lateout("a0") ret,
        );
    }
    ret
}

#[inline(always)]
fn syscall1(num: usize, a0: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!(
            "ecall",
            in("a7") num,
            inlateout("a0") a0 => ret,
        );
    }
    ret
}

#[inline(always)]
fn syscall2(num: usize, a0: usize, a1: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!(
            "ecall",
            in("a7") num,
            inlateout("a0") a0 => ret,
            in("a1") a1,
        );
    }
    ret
}

#[inline(always)]
fn syscall3(num: usize, a0: usize, a1: usize, a2: usize) -> usize {
    let ret: usize;
    unsafe {
        asm!(
            "ecall",
            in("a7") num,
            inlateout("a0") a0 => ret,
            in("a1") a1,
            in("a2") a2,
        );
    }
    ret
}

pub fn fork() -> usize {
    syscall0(Syscall::Fork as usize)
}

pub fn exit(code: usize) -> ! {
    syscall1(SYS_EXIT, code);
    unreachable!();
}

pub fn wait(status: &mut usize) -> usize {
    syscall1(SYS_WAIT, status as *mut usize as usize)
}

pub fn pipe(fds: &mut [usize; 2]) -> usize {
    syscall1(SYS_PIPE, fds.as_mut_ptr() as usize)
}

pub fn read(fd: usize, buf: &mut [u8]) -> usize {
    syscall3(SYS_READ, fd, buf.as_mut_ptr() as usize, buf.len())
}

pub fn kill(pid: usize) -> usize {
    syscall1(SYS_KILL, pid)
}

pub fn exec(path: &str, argv: &[&str]) -> usize {
    syscall2(SYS_EXEC, path.as_ptr() as usize, argv.as_ptr() as usize)
}

pub fn fstat(fd: usize, stat: *mut u8) -> usize {
    syscall2(SYS_FSTAT, fd, stat as usize)
}

pub fn chdir(path: &[u8]) -> usize {
    syscall1(SYS_CHDIR, path.as_ptr() as usize)
}

pub fn dup(fd: usize) -> usize {
    syscall1(SYS_DUP, fd)
}

pub fn getpid() -> usize {
    syscall0(SYS_GETPID)
}

pub fn sbrk(n: isize) -> usize {
    syscall1(SYS_SBRK, n as usize)
}

pub fn sleep(ticks: usize) -> usize {
    syscall1(SYS_SLEEP, ticks)
}

pub fn uptime() -> usize {
    syscall0(SYS_UPTIME)
}

pub fn open(path: &[u8], flags: usize) -> usize {
    syscall2(SYS_OPEN, path.as_ptr() as usize, flags)
}

pub fn write(fd: usize, buf: &[u8]) -> usize {
    syscall3(SYS_WRITE, fd, buf.as_ptr() as usize, buf.len())
}

pub fn mknod(path: &[u8], major: usize, minor: usize) -> usize {
    syscall3(SYS_MKNOD, path.as_ptr() as usize, major, minor)
}

pub fn unlink(path: &[u8]) -> usize {
    syscall1(SYS_UNLINK, path.as_ptr() as usize)
}

pub fn link(old: &[u8], new: &[u8]) -> usize {
    syscall2(SYS_LINK, old.as_ptr() as usize, new.as_ptr() as usize)
}

pub fn mkdir(path: &[u8]) -> usize {
    syscall1(SYS_MKDIR, path.as_ptr() as usize)
}

pub fn close(fd: usize) -> usize {
    syscall1(SYS_CLOSE, fd)
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(1)
}
