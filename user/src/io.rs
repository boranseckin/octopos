use kernel::abi::SysError;

use crate::syscall::{check, check_unit, raw};

pub use crate::line::LineEditor;
pub use kernel::abi::{CONSOLE, Ioctl};

/// A file descriptor returned by or passed to syscalls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fd(pub usize);

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
        core::fmt::Display::fmt(&self.0, f)
    }
}

pub fn dup(fd: Fd) -> Result<Fd, SysError> {
    check(raw::dup(fd.as_raw())).map(Fd)
}

pub fn close(fd: Fd) -> Result<(), SysError> {
    check_unit(raw::close(fd.as_raw()))
}

pub fn ioctl(fd: Fd, cmd: usize, arg: usize) -> Result<usize, SysError> {
    check(raw::ioctl(fd.as_raw(), cmd, arg))
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

/// Implemented by readable byte sources, analogous to `std::io::Read`.
pub trait Read {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, SysError>;

    /// Reads until `buf` is fully filled, retrying on short reads.
    /// Returns `SysError::IoError` if EOF is reached before `buf` is exhausted.
    fn read_exact(&mut self, mut buf: &mut [u8]) -> Result<(), SysError> {
        while !buf.is_empty() {
            let n = self.read(buf)?;
            if n == 0 {
                return Err(SysError::IoError);
            }
            buf = &mut buf[n..];
        }
        Ok(())
    }
}

/// Implemented by writable byte sinks, analogous to `std::io::Write`.
pub trait Write {
    fn write(&mut self, buf: &[u8]) -> Result<usize, SysError>;

    /// Writes all of `buf`, retrying after partial writes.
    fn write_all(&mut self, mut buf: &[u8]) -> Result<(), SysError> {
        while !buf.is_empty() {
            let n = self.write(buf)?;
            buf = &buf[n..];
        }
        Ok(())
    }
}

/// Any `Fd` implements both `Read` and `Write` so it can be passed wherever a
/// generic reader or writer is expected (e.g. `fn cat(src: &mut impl Read)`).
impl Read for Fd {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, SysError> {
        read(*self, buf)
    }
}

impl Write for Fd {
    fn write(&mut self, buf: &[u8]) -> Result<usize, SysError> {
        write(*self, buf)
    }
}

pub struct Stdin;

impl Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, SysError> {
        read(Fd::STDIN, buf)
    }
}

pub struct Stdout;

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> Result<usize, SysError> {
        write(Fd::STDOUT, buf)
    }
}

/// `core::fmt::Write` delegates to our binary `Write` impl, so the `print!`
/// and `write!` formatting macros share the same code path as `write_all`.
impl core::fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_all(s.as_bytes()).map_err(|_| core::fmt::Error)
    }
}

pub struct Stderr;

impl Write for Stderr {
    fn write(&mut self, buf: &[u8]) -> Result<usize, SysError> {
        write(Fd::STDERR, buf)
    }
}

impl core::fmt::Write for Stderr {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_all(s.as_bytes()).map_err(|_| core::fmt::Error)
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        <$crate::io::Stdout as core::fmt::Write>::write_fmt(
            &mut $crate::io::Stdout,
            format_args!($($arg)*),
        ).unwrap()
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };

    ($($arg:tt)*) => {
        $crate::print!("{}\n", core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => {
        <$crate::io::Stderr as core::fmt::Write>::write_fmt(
            &mut $crate::io::Stderr,
            format_args!($($arg)*),
        ).unwrap()
    };
}

#[macro_export]
macro_rules! eprintln {
    () => {
        $crate::eprint!("\n")
    };

    ($($arg:tt)*) => {
        $crate::eprint!("{}\n", core::format_args!($($arg)*))
    };
}
