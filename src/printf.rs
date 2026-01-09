use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering};

use crate::console;
use crate::proc::CPU_POOL;
use crate::spinlock::SpinLock;

/// Wrapper around console writer
/// Only used to gate writing behind a mutex lock
struct Writer;

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            console::putc(byte);
        }
        Ok(())
    }
}

pub static PRINTF: Printf = Printf {
    writer: SpinLock::new(Writer, "printf"),
    locking: AtomicBool::new(true),
    panicked: AtomicBool::new(false),
};

pub struct Printf {
    writer: SpinLock<Writer>,
    locking: AtomicBool,
    panicked: AtomicBool,
}

impl Printf {
    pub fn is_panicked(&self) -> bool {
        self.panicked.load(Ordering::Relaxed)
    }
}

pub fn _print(args: fmt::Arguments<'_>) {
    let writer = if PRINTF.locking.load(Ordering::Relaxed) {
        &mut *PRINTF.writer.lock()
    } else {
        // We are panicked, don't care about the lock
        unsafe { PRINTF.writer.get_mut_unchecked() }
    };

    writer.write_fmt(args).unwrap();
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        $crate::printf::_print(format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! println {
    ($fmt:literal $(,$($arg:tt)+)?) => {
        $crate::printf::_print(format_args!(concat!($fmt, "\n") $(,$($arg)+)?))
    };
}

pub fn panic(info: &core::panic::PanicInfo) -> ! {
    PRINTF.locking.store(false, Ordering::Relaxed);

    // Safety: we are panicked, don't care about the lock
    let cpu_id = unsafe { CPU_POOL.current_id() };

    println!("hart {} {}", cpu_id, info);

    PRINTF.panicked.store(true, Ordering::Relaxed);

    #[allow(clippy::empty_loop)]
    loop {}
}
