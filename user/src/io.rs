use kernel::abi::SysError;

use crate::syscall::{self, Fd};

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
        syscall::read(*self, buf)
    }
}

impl Write for Fd {
    fn write(&mut self, buf: &[u8]) -> Result<usize, SysError> {
        syscall::write(*self, buf)
    }
}

pub struct Stdin;

impl Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, SysError> {
        syscall::read(Fd::STDIN, buf)
    }
}

pub struct Stdout;

impl Write for Stdout {
    fn write(&mut self, buf: &[u8]) -> Result<usize, SysError> {
        syscall::write(Fd::STDOUT, buf)
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
        syscall::write(Fd::STDERR, buf)
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
        <$crate::Stdout as core::fmt::Write>::write_fmt(
            &mut $crate::Stdout,
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
        $crate::print!("{}\n", format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => {
        <$crate::Stderr as core::fmt::Write>::write_fmt(
            &mut $crate::Stderr,
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
        $crate::eprint!("{}\n", format_args!($($arg)*))
    };
}
