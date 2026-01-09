use core::num::Wrapping;

use crate::spinlock::SpinLock;
use crate::uart;

const INPUT_BUF_SIZE: usize = 128;

pub static CONSOLE: SpinLock<Console> = SpinLock::new(Console::new(), "console");

pub struct Console {
    buf: [u8; INPUT_BUF_SIZE],
    r: Wrapping<usize>,
    w: Wrapping<usize>,
    e: Wrapping<usize>,
}

impl Console {
    const fn new() -> Self {
        Self {
            buf: [0; INPUT_BUF_SIZE],
            r: Wrapping(0),
            w: Wrapping(0),
            e: Wrapping(0),
        }
    }
}

pub fn putc(c: u8) {
    uart::putc_sync(c);
}

pub fn init() {
    unsafe { uart::init() };

    // TODO: system calls
}
