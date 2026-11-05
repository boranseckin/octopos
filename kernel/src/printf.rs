use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::proc;
use crate::spinlock::SpinLock;
use crate::uart;

/// Wrapper around `uart::write_sync` that implements `fmt::Write`.
///
/// Held behind `Printf::writer` so that concurrent kernel `print!` calls do not interleave their
/// formatted output across multiple `write_str` calls.
pub struct Writer;

impl Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        uart::write_sync(s.as_bytes());
        Ok(())
    }
}

pub static PRINTF: Printf = Printf {
    writer: SpinLock::new(Writer, "printf"),
    panicking: AtomicBool::new(false),
    panicked: AtomicBool::new(false),
};

pub struct Printf {
    /// Serializes concurrent kernel `print!` calls so their output does not interleave.
    writer: SpinLock<Writer>,
    /// Set to true before the panic message is printed.
    ///
    /// Causes `print()` to bypass `writer` (avoiding a deadlock if the panicking hart already
    /// holds it) and causes `uart::write()` to abort and release `UART.lock()` so that
    /// `uart::write_sync()` can acquire it to print the panic message.
    panicking: AtomicBool,
    /// Set to true after the panic message has been printed.
    ///
    /// Causes `uart::write_sync()` to freeze all harts, preventing any output from appearing
    /// after the panic message.
    panicked: AtomicBool,
}

impl Printf {
    pub fn is_panicking(&self) -> bool {
        self.panicking.load(Ordering::Relaxed)
    }

    pub fn is_panicked(&self) -> bool {
        self.panicked.load(Ordering::Relaxed)
    }
}

/// Prints formatted output to the console.
///
/// Acquires `PRINTF.writer` to prevent interleaving with concurrent kernel prints. When panicking,
/// bypasses the lock to avoid deadlocking if this hart already holds it.
pub fn print(args: fmt::Arguments<'_>) {
    let writer = if !PRINTF.is_panicking() {
        &mut *PRINTF.writer.lock()
    } else {
        // Safety: interrupts are disabled by the caller or we are panicking on a single hart.
        unsafe { PRINTF.writer.get_mut_unchecked() }
    };

    writer.write_fmt(args).unwrap();
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        $crate::printf::print(format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! println {
    ($fmt:literal $(,$($arg:tt)+)?) => {
        $crate::printf::print(format_args!(concat!($fmt, "\n") $(,$($arg)+)?))
    };
}

/// Taken from std::dbg!.
#[macro_export]
macro_rules! dbg {
    () => {
        $crate::println!("[{}:{}:{}]", file!(), line!(), column!())
    };
    ($val:expr $(,)?) => {
        match $val {
            tmp => {
                $crate::println!("[{}:{}:{}] {} = {:#?}",
                    core::file!(),
                    core::line!(),
                    core::column!(),
                    core::stringify!($val),
                    &&tmp as &dyn core::fmt::Debug,
                );
                tmp
            }
        }
    };
    ($($val:expr),+ $(,)?) => {
        ($($crate::dbg!($val)),+,)
    };
}

/// Stack buffer used to pre-format the panic message before writing it to the UART.
///
/// Pre-formatting lets `uart::write_sync()` be called once with the complete message, so
/// `UART.lock()` is held for the entire output and user-space writes cannot interleave.
struct PanicBuffer<const N: usize> {
    data: [u8; N],
    len: usize,
}

impl<const N: usize> Write for PanicBuffer<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let space = self.data.len() - self.len;
        let n = bytes.len().min(space);
        self.data[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
        Ok(())
    }
}

pub fn panic(info: &core::panic::PanicInfo) -> ! {
    PRINTF.panicking.store(true, Ordering::Relaxed);

    // Safety: normally requires interrupts disabled to prevent the hart id from becoming stale
    // due to preemption, but in a panic context a stale value is acceptable.
    let cpu_id = unsafe { proc::current_id() };

    let mut buf = PanicBuffer::<256> {
        data: [0; 256],
        len: 0,
    };
    let _ = writeln!(buf, "\n! hart {} {}", cpu_id, info);
    uart::write_sync(&buf.data[..buf.len]);

    PRINTF.panicked.store(true, Ordering::Relaxed);

    loop {
        core::hint::spin_loop();
    }
}

/// Fixed-capacity string buffer for formatting without allocation.
struct StackStr<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> StackStr<N> {
    fn new() -> Self {
        Self {
            buf: [0u8; N],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        // SAFETY: buf[..len] is a concatenation of whole `&str`s written by `write_str`
        unsafe { str::from_utf8_unchecked(&self.buf[..self.len]) }
    }
}

impl<const N: usize> fmt::Write for StackStr<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len + s.len();

        if end > N {
            return Err(fmt::Error);
        }

        self.buf[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;

        Ok(())
    }
}

/// Writes `args` to `f`, honouring width, alignment and precision.
/// Formats into an `N`-byte stack buffer when padding is requested;
/// `N` must be at least the longest possible output, or formatting fails.
pub fn pad_args<const N: usize>(
    f: &mut fmt::Formatter<'_>,
    args: fmt::Arguments<'_>,
) -> fmt::Result {
    if f.width().is_none() && f.precision().is_none() {
        return f.write_fmt(args);
    }

    let mut buf = StackStr::<N>::new();
    fmt::write(&mut buf, args)?;
    f.pad(buf.as_str())
}
