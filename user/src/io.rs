use crate::syscall::write;

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
