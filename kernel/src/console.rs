use crate::proc::{self, Channel};
use crate::spinlock::SpinLock;
use crate::syscall::SyscallError;
use crate::uart;
use crate::vm::VA;

/// Translate character to control-key equivalent.
const fn ctrl(c: u8) -> u8 {
    c.wrapping_sub(b'@')
}

const INPUT_BUF_SIZE: usize = 128;

pub static CONSOLE: SpinLock<Console> = SpinLock::new(Console::new(), "console");

/// Console structure
pub struct Console {
    buf: [u8; INPUT_BUF_SIZE],
    /// read index
    r: usize,
    /// write index (completed input)
    w: usize,
    /// edit index (current editign position)
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

    /// Outputs a character to the console.
    pub fn putc(c: u8) {
        uart::putc_sync(c);
    }

    /// Handles backspace by erasing the character before the cursor.
    pub fn put_backspace() {
        Self::putc(b'\x08'); // backspace
        Self::putc(b' '); // over-write with space
        Self::putc(b'\x08'); // backsapce again
    }

    /// User `write()`s to the console are handled here.
    /// Currently only handles user addresses.
    pub fn write(src: VA, len: usize) -> Result<usize, SyscallError> {
        // TODO: avoid byte to byte copy_in and use chunks
        for i in 0..len {
            let src = VA::from(src.as_usize() + i);
            let mut dst = [0u8];

            match proc::copy_from_user(src, &mut dst) {
                Ok(_) => Self::putc(dst[0]),
                Err(_) => return Ok(i),
            }
        }

        Ok(len)
    }

    /// User `read()`s from the console are handled here.
    /// Currently only handles user addresses.
    pub fn read(dst: VA, mut len: usize) -> Result<usize, SyscallError> {
        let mut console = CONSOLE.lock();

        let mut dst = dst;
        let target = len;

        while len > 0 {
            // wait until interrupt handler has put some input into `buf`.
            while console.r == console.w {
                if proc::current_proc().is_killed() {
                    return Err(SyscallError::Console);
                }

                console = proc::sleep(Channel::Buffer(&console.r as *const _ as usize), console);
            }

            let index = console.r % INPUT_BUF_SIZE;
            let c = console.buf[index];
            console.r += 1;

            // end-of-file
            if c == ctrl(b'D') {
                if len < target {
                    // save ^D for next time, to make sure caller gets a 0-byte result
                    console.r -= 1;
                }

                break;
            }

            // copy the input byte to the user-space buffer
            let buf = [c];
            if proc::copy_to_user(&buf, dst).is_err() {
                break;
            }

            dst = VA::from(dst.as_usize() + 1);
            len -= 1;

            // a whole line has arrived, return to the user-level `read()`
            if c == b'\n' {
                break;
            }
        }

        Ok(target - len)
    }

    /// Console input interrupt handler.
    ///
    /// `uart_intr()` calls this for input character.
    /// Does erase/kill processing, append to `buf`, and wakes up `read()` if a whole line has arrived.
    pub fn handle_interrupt(c: u8) {
        let mut console = CONSOLE.lock();

        match c {
            // backspace or delete
            c if c == ctrl(b'H') || c == b'\x7f' => {
                if console.e != console.w {
                    console.e -= 1;
                    Console::put_backspace();
                }
            }

            // normal character
            mut c => {
                if c != 0 && console.e - console.r < INPUT_BUF_SIZE {
                    if c == b'\r' {
                        c = b'\n';
                    }

                    // echo back to the user
                    Console::putc(c);

                    // store for consumption by `read()`
                    let index = console.e % INPUT_BUF_SIZE;
                    console.buf[index] = c;
                    console.e += 1;

                    // new line or carriage return or end up of buffer
                    if c == b'\n' || c == ctrl(b'D') || console.e - console.r == INPUT_BUF_SIZE {
                        // wake up `read` if a whole line (or end-of-file) has arrived
                        console.w = console.e;
                        proc::wakeup(Channel::Buffer(&console.r as *const _ as usize));
                    }
                }
            }
        }
    }
}

/// Initialize console and system calls.
///
/// # Safety
/// Must be called only once during kernel initialization.
pub unsafe fn init() {
    unsafe { uart::init() };
}
