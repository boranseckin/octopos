use core::num::Wrapping;

use crate::spinlock::SpinLock;
use crate::uart;

const INPUT_BUF_SIZE: usize = 128;

pub static CONSOLE: SpinLock<Console> = SpinLock::new(Console::new(), "console");

pub struct Console {
    _buf: [u8; INPUT_BUF_SIZE],
    _r: Wrapping<usize>,
    _w: Wrapping<usize>,
    _e: Wrapping<usize>,
}

impl Console {
    const fn new() -> Self {
        Self {
            _buf: [0; INPUT_BUF_SIZE],
            _r: Wrapping(0),
            _w: Wrapping(0),
            _e: Wrapping(0),
        }
    }
}

pub fn putc(c: u8) {
    uart::putc_sync(c);
}

/// Initialize console and system calls.
///
/// # Safety
/// Must be called only once during kernel initialization.
pub unsafe fn init() {
    unsafe { uart::init() };

    // TODO: system calls
}
