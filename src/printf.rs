use core::fmt::{self, Write};
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::console;
use crate::proc::Cpus;
use crate::spinlock::Mutex;

pub static PRINTF: Printf = Printf {
    writer: Mutex::new(Writer, "printf"),
    locking: AtomicBool::new(true),
    panicked: AtomicBool::new(false),
};

pub struct Printf {
    writer: Mutex<Writer>,
    locking: AtomicBool,
    panicked: AtomicBool,
}

impl Printf {
    pub fn is_panicked(&self) -> &AtomicBool {
        &self.panicked
    }
}

pub struct Writer;

impl Writer {
    fn print(&self, c: u8) {
        console::putc(c)
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.print(byte);
        }
        Ok(())
    }
}

pub fn _print(args: fmt::Arguments<'_>, newline: bool) {
    if PRINTF.locking.load(Ordering::Relaxed) {
        let mut lock = PRINTF.writer.lock();

        lock.write_fmt(args).expect("print error");
        if newline {
            lock.write_char('\n').expect("print error nl");
        }
    } else {
        // We are panicked, don't care about the lock
        unsafe {
            let writer = PRINTF.writer.get_mut_unchecked();

            writer.write_fmt(args).unwrap();
            if newline {
                writer.write_char('\n').unwrap();
            }
        }
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        $crate::printf::_print(format_args!($($arg)*), false);
    }};
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n");
    };
    ($($arg:tt)*) => {{
        $crate::printf::_print(format_args!($($arg)*), true);
    }};
}

static mut EWRITER: Writer = Writer;

pub fn _eprint(args: fmt::Arguments<'_>) {
    unsafe {
        EWRITER.write_fmt(args).unwrap();
    }
}

#[macro_export]
macro_rules! eprint {
    ($($arg:tt)*) => {{
        $crate::printf::_eprint(format_args!($($arg)*));
    }};
}

pub fn handle_panic(info: &PanicInfo) -> ! {
    PRINTF.locking.store(false, Ordering::Relaxed);

    let cpu_id = unsafe { Cpus::get_id() };
    println!("hart {cpu_id} {info}");

    PRINTF.panicked.store(true, Ordering::Relaxed);

    #[allow(clippy::empty_loop)]
    loop {}
}
