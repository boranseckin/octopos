use crate::file::Ioctl;
use crate::proc::{self, Channel, PROC_TABLE, Pid};
use crate::spinlock::SpinLock;
use crate::syscall::SysError;
use crate::uart;
use crate::vm::VA;

/// Translate character to control-key equivalent.
const fn ctrl(c: u8) -> u8 {
    c.wrapping_sub(b'@')
}

const INPUT_BUF_SIZE: usize = 128;

pub static CONSOLE: SpinLock<Console> = SpinLock::new(Console::new(), "console");

/// Console structure
///
/// The buf is a circular array of 128 bytes. The three indices mark three zones within it, and the
/// invariant is always `r <= w <= e` (mod 128):
///
/// buf: \[ consumed | completed lines | line being edited \]
/// ----------------^r----------------^w------------------^e
///
/// `e` (edit): the tip of what the user has typed so far. Every character from the interrupt
/// handler appends to `buf[e % 128]` then increments `e`. Backspace decrements `e`. `Ctrl-U` walks
/// `e` back to `w`. Nothing between `w` and `e` is visible to `read()` yet, it's "in flight" in the
/// editor.
///
/// `w` (write): the boundary of what's been committed as a complete unit. When `handle_interrupt`
/// sees `\n`, `Ctrl-D`, or a full buffer, it sets `w = e`, which atomically publishes everything
/// typed so far and wakes up any sleeping `read()`. Only moves forward, never back.
///
/// `r` (read): where user-space left off consuming. `read()` sleeps while `r == w` (nothing
/// committed). It pops `buf[r % 128]` and increments `r` until it hits `w` or a `\n`.
pub struct Console {
    buf: [u8; INPUT_BUF_SIZE],
    /// read index
    r: usize,
    /// write index (completed input)
    w: usize,
    /// edit index (current editign position)
    e: usize,
    /// raw mode: if true, input is not processed
    raw: bool,
    /// pid of process that has the console as its foreground device
    foreground_pid: Option<Pid>,
}

impl Console {
    const fn new() -> Self {
        Self {
            buf: [0; INPUT_BUF_SIZE],
            r: 0,
            w: 0,
            e: 0,
            raw: false,
            foreground_pid: None,
        }
    }

    /// Outputs a character to the console.
    pub fn putc_sync(c: u8) {
        uart::write_sync(&[c]);
    }

    /// Handles backspace by erasing the character before the cursor.
    pub fn put_backspace() {
        Self::putc_sync(b'\x08'); // backspace
        Self::putc_sync(b' '); // over-write with space
        Self::putc_sync(b'\x08'); // backsapce again
    }

    /// User `write()`s to the console are handled here.
    pub fn write(mut src: VA, len: usize) -> Result<usize, SysError> {
        let mut n = 0;

        let mut buf = [0u8; 32];

        let raw = {
            let console = CONSOLE.lock();
            console.raw
        };

        while n < len {
            let chunk = 32.min(len - n);
            match proc::copy_from_user(src, &mut buf[..chunk]) {
                Ok(_) => {
                    // in raw mode, we are using write_sync to avoid sleeping between characters.
                    // this gets rid of flickers but could cause longer blocking against the kernel
                    if raw {
                        uart::write_sync(&buf[..chunk]);
                    } else {
                        uart::write(&buf[..chunk]);
                    }
                    n += chunk;
                    src += chunk;
                }
                Err(_) => return Ok(n),
            }
        }

        Ok(len)
    }

    /// User `read()`s from the console are handled here.
    /// Currently only handles user addresses.
    pub fn read(mut dst: VA, mut len: usize) -> Result<usize, SysError> {
        let mut console = CONSOLE.lock();

        let target = len;

        while len > 0 {
            // wait until interrupt handler has put some input into `buf`.
            while console.r == console.w {
                if proc::current_proc().is_killed() {
                    err!(SysError::Interrupted);
                }

                console = proc::sleep(Channel::Buffer(&console.r as *const _ as usize), console);
            }

            let index = console.r % INPUT_BUF_SIZE;
            let c = console.buf[index];
            console.r += 1;

            // handle Ctrl-D EOF; only on cooked mode
            if !console.raw && c == ctrl(b'D') {
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

            dst += 1;
            len -= 1;

            // a whole line has arrived or in raw mode, return to the user-level `read()`
            if c == b'\n' || console.raw {
                break;
            }
        }

        Ok(target - len)
    }

    /// Console input interrupt handler.
    ///
    /// `uart::handle_interrupt()` calls this for each input character.
    /// Does erase/kill processing, append to `buf`, and wakes up `read()` if a whole line has
    /// arrived.
    pub fn handle_interrupt(c: u8) {
        let mut console = CONSOLE.lock();

        // Raw mode: store the character and wake up `read()` immediately, without processing.
        if console.raw && c != 0 && console.e - console.r < INPUT_BUF_SIZE {
            let index = console.e % INPUT_BUF_SIZE;
            console.buf[index] = c;
            console.e += 1;
            console.w = console.e; // wake up `read()` for each character
            proc::wakeup(Channel::Buffer(&console.r as *const _ as usize));
            return;
        }

        // Cooked mode: handle special characters and only wake up `read()` when a whole line has
        // arrived.
        match c {
            // backspace or delete
            c if c == ctrl(b'H') || c == b'\x7f' => {
                if console.e != console.w {
                    console.e -= 1;
                    Console::put_backspace();
                }
            }

            // print process list
            c if c == ctrl(b'P') => {
                unsafe { PROC_TABLE.dump() };
            }

            c if c == ctrl(b'R') => {
                crate::net::interface::dump();
                crate::net::route::dump();
                crate::net::udp::dump();
            }

            // kill the line
            c if c == ctrl(b'U') => {
                while console.e != console.w
                    && console.buf[(console.e - 1) % INPUT_BUF_SIZE] != b'\n'
                {
                    console.e -= 1;
                    Console::put_backspace();
                }
            }

            // kill the process if any
            c if c == ctrl(b'C') => {
                if let Some(pid) = console.foreground_pid {
                    proc::kill(pid);
                }
            }

            // normal character
            mut c => {
                if c != 0 && console.e - console.r < INPUT_BUF_SIZE {
                    if c == b'\r' {
                        c = b'\n';
                    }

                    // echo back to the user, this does not sleep
                    Self::putc_sync(c);

                    // store for consumption by `read()`
                    let index = console.e % INPUT_BUF_SIZE;
                    console.buf[index] = c;
                    console.e += 1;

                    // new line or carriage return or end up of buffer
                    if c == b'\n' || c == ctrl(b'D') || console.e - console.r == INPUT_BUF_SIZE {
                        // wake up `read()` if a whole line (or end-of-file) has arrived
                        console.w = console.e;
                        proc::wakeup(Channel::Buffer(&console.r as *const _ as usize));
                    }
                }
            }
        }
    }

    pub fn ioctl(cmd: usize, arg: usize) -> Result<usize, SysError> {
        let mut console = CONSOLE.lock();
        match cmd {
            Ioctl::CONSOLE_SET_RAW => {
                if arg == 1 {
                    console.raw = true;
                    console.e = console.w;
                } else {
                    console.raw = false;
                }

                Ok(0)
            }
            Ioctl::CONSOLE_SET_FG_PID => {
                if arg == 0 {
                    console.foreground_pid = None;
                } else {
                    // # Safety: we trust the caller to pass a valid PID
                    console.foreground_pid = Some(unsafe { Pid::from_usize(arg) });
                }

                Ok(0)
            }
            _ => Err(SysError::InvalidArgument),
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
