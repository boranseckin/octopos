use crate::{spinlock::Mutex, uart};

const INPUT_BUF_SIZE: usize = 128;

const BACKSPACE: u8 = 0x08;
const fn ctrl(c: u8) -> u8 {
    c - b'@'
}

pub static CONSOLE: Mutex<Console> = Mutex::new(Console::new(), "console");

pub struct Console {
    buf: [u8; INPUT_BUF_SIZE],
    r: usize,
    w: usize,
    e: usize,
}

impl Console {
    const fn new() -> Self {
        Self {
            buf: [0; INPUT_BUF_SIZE],
            r: 0,
            w: 0,
            e: 0,
        }
    }
}

pub fn putc(c: u8) {
    if c == BACKSPACE {
        uart::putc_sync(BACKSPACE);
        uart::putc_sync(b' ');
        uart::putc_sync(BACKSPACE);
    } else {
        uart::putc_sync(c);
    }
}

pub fn init() {
    unsafe { uart::init() };

    // TODO: system calls
}
