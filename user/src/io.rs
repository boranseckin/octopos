use core::str;

use crate::syscall::{read, write};

pub struct Stdout;

impl core::fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let len = write(1, s.as_bytes());
        if len == s.len() {
            Ok(())
        } else {
            Err(core::fmt::Error)
        }
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        <$crate::Stdout as core::fmt::Write>::write_fmt(
            &mut $crate::Stdout,
            format_args!($($arg)*),
        ).unwrap();
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

pub struct Stderr;

impl core::fmt::Write for Stderr {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let len = write(2, s.as_bytes());
        if len == s.len() {
            Ok(())
        } else {
            Err(core::fmt::Error)
        }
    }
}

#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => {
        <$crate::Stderr as core::fmt::Write>::write_fmt(
            &mut $crate::Stderr,
            format_args!($($arg)*),
        ).unwrap();
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

/// Reads a line from `stdin` into `buf`.
/// Returns slice of what was read.
pub fn gets(buf: &mut [u8]) -> Option<&str> {
    let mut i = 0;

    while i < buf.len() - 1 {
        let mut c = [0u8; 1];
        if read(0, &mut c) != 1 {
            return None; // EOF or error
        }

        if c[0] == b'\n' || c[0] == b'\r' {
            break;
        }

        buf[i] = c[0];
        i += 1;
    }

    buf[i] = 0; // null terminate
    str::from_utf8(&buf[..i]).ok()
}
