use kernel::abi::Ioctl;

use crate::io::{Fd, Read, Stderr, Stdin, Stdout, Write, ioctl};

/// A line editor for reading user input from the console, supporting basic editing
/// and a history of previous lines entered.
#[derive(Debug, Clone, Copy)]
pub struct LineEditor<'a> {
    /// buffer for the current line being edited
    buf: [u8; LineEditor::LINE_MAX],
    /// how many bytes are in the `buf`
    len: usize,
    /// index of the character being edited in the `buf`
    cursor: usize,
    /// the prompt to display before the line editor
    prompt: &'a str,

    /// circular buffer of previous lines entered
    history: [[u8; LineEditor::LINE_MAX]; LineEditor::HISTORY_SIZE],
    /// length of each entry in `history`
    history_lens: [usize; LineEditor::HISTORY_SIZE],
    /// number of entries in `history`, circular index
    history_entries: usize,
    /// index of the current entry in `history` being displayed
    /// 0 = current line, 1 = most-recent entry
    history_offset: usize,
    /// buffer for stashing the current line when navigating history
    stashed_buf: [u8; LineEditor::LINE_MAX],
    /// length of the stashed line
    stashed_len: usize,
}

impl<'a> LineEditor<'a> {
    const LINE_MAX: usize = 256;
    const HISTORY_SIZE: usize = 16;

    pub fn new() -> Self {
        Self {
            buf: [0; Self::LINE_MAX],
            len: 0,
            cursor: 0,
            prompt: "",
            history: [[0; Self::LINE_MAX]; Self::HISTORY_SIZE],
            history_lens: [0; Self::HISTORY_SIZE],
            history_entries: 0,
            history_offset: 0,
            stashed_buf: [0; Self::LINE_MAX],
            stashed_len: 0,
        }
    }

    pub fn read_line(&mut self, prompt: &'a str) -> Option<&str> {
        ioctl(Fd::STDIN, Ioctl::CONSOLE_SET_RAW, 1).expect("failed to set console to raw mode");

        self.len = 0;
        self.cursor = 0;

        self.prompt = prompt;
        Stderr.write_all(self.prompt.as_bytes()).unwrap();

        let mut c = [0u8; 1];
        loop {
            Stdin.read_exact(&mut c).unwrap();

            match c[0] {
                // enter
                b'\n' | b'\r' => {
                    Stdout.write_all(b"\r\n").unwrap();
                    break;
                }

                // backspace or delete
                b'\x08' | b'\x7f' => {
                    self.backspace();
                }

                // start of escape sequence
                b'\x1b' => {
                    self.handle_escape();
                }

                // Ctrl-A
                b'\x01' => {
                    self.move_to_start();
                }

                // Ctrl-E
                b'\x05' => {
                    self.move_to_end();
                }

                // Ctrl-U
                b'\x15' => {
                    self.kill_line();
                }

                // Ctrl-W
                b'\x17' => {
                    self.kill_word();
                }

                // Ctrl-L
                b'\x0c' => {
                    self.redraw_full();
                }

                // Ctrl-D
                b'\x04' if self.len == 0 => {
                    Stdout.write_all(b"\r\n").unwrap();
                    return None;
                }

                // Ctrl-C
                b'\x03' => {
                    Stdout.write_all(b"^C\r\n").unwrap();
                    self.len = 0;
                    self.cursor = 0;
                    break;
                }

                // normal character
                c if c.is_ascii_graphic() || c == b' ' => {
                    self.insert(c);
                }

                _ => {}
            }
        }

        ioctl(Fd::STDIN, Ioctl::CONSOLE_SET_RAW, 0).expect("failed to set console to cooked mode");

        if self.len > 0 {
            self.add_to_history();
        }
        self.history_offset = 0;

        Some(unsafe { str::from_utf8_unchecked(&self.buf[..self.len]) })
    }

    /// Called when `0x1b` is read.
    /// Reads the next two bytes and dispatches the correct handler.
    fn handle_escape(&mut self) {
        let mut seq = [0u8; 2];
        Stdin.read_exact(&mut seq).unwrap();

        match seq {
            [b'[', b'A'] => self.history_up(),
            [b'[', b'B'] => self.history_down(),
            [b'[', b'D'] => self.move_left(),
            [b'[', b'C'] => self.move_right(),
            _ => {}
        }
    }

    fn insert(&mut self, c: u8) {
        if self.len >= Self::LINE_MAX {
            return;
        }

        // shift buf[cursor..len] right by 1
        for i in (self.cursor..self.len).rev() {
            self.buf[i + 1] = self.buf[i];
        }

        // place c at cursor
        self.buf[self.cursor] = c;

        self.cursor += 1;
        self.len += 1;

        self.redraw();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }

        // shift buf[cursor..len] left by 1
        for i in (self.cursor - 1)..(self.len - 1) {
            self.buf[i] = self.buf[i + 1];
        }

        self.cursor -= 1;
        self.len -= 1;

        self.redraw();
    }

    fn kill_line(&mut self) {
        // shift buf[cursor..len] to buf[0..]
        for i in self.cursor..self.len {
            self.buf[i - self.cursor] = self.buf[i];
        }

        self.len -= self.cursor;
        self.cursor = 0;

        self.redraw();
    }

    fn kill_word(&mut self) {
        let mut i = self.cursor;

        // skip over any spaces before a word
        while i > 0 && self.buf[i - 1] == b' ' {
            i -= 1;
        }

        // skip over the first word
        while i > 0 && self.buf[i - 1] != b' ' {
            i -= 1;
        }

        // shift buf[cursor..len] left by cursor - i
        for j in self.cursor..self.len {
            self.buf[j - (self.cursor - i)] = self.buf[j];
        }

        self.len -= self.cursor - i;
        self.cursor = i;

        self.redraw();
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;

            // emit \x1b[D to move cursor left
            Stdout.write_all(b"\x1b[D").unwrap();
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.len {
            self.cursor += 1;

            // emit \x1b[C to move cursor right
            Stdout.write_all(b"\x1b[C").unwrap();
        }
    }

    fn move_to_start(&mut self) {
        self.cursor = 0;
        self.redraw();
    }

    fn move_to_end(&mut self) {
        self.cursor = self.len;
        self.redraw();
    }

    fn redraw(&self) {
        // Worst case: "\r" (1) + prompt + buf (256) + "\x1b[K" (3) + "\x1b[999D" (7)
        let mut out = [0u8; 512];
        let mut n = 0;

        // move cursor to start of line
        out[n] = b'\r';
        n += 1;

        // prompt
        let prompt = self.prompt.as_bytes();
        out[n..n + prompt.len()].copy_from_slice(prompt);
        n += prompt.len();

        // line content
        out[n..n + self.len].copy_from_slice(&self.buf[..self.len]);
        n += self.len;

        // erase trailing characters from previous longer line
        out[n..n + 3].copy_from_slice(b"\x1b[K");
        n += 3;

        // move cursor to correct position if it isn't already
        let back = self.len - self.cursor;
        if back > 0 {
            out[n] = b'\x1b';
            out[n + 1] = b'[';
            n += 2;
            n += write_decimal(&mut out[n..], back);
            out[n] = b'D';
            n += 1;
        }

        Stdout.write_all(&out[..n]).unwrap();
    }

    fn redraw_full(&self) {
        // \x1b[2J clears the entire screen, \x1b[H moves cursor to top-left
        Stdout.write_all(b"\x1b[2J\x1b[H").unwrap();
        self.redraw();
    }

    fn add_to_history(&mut self) {
        let slot = self.history_entries % Self::HISTORY_SIZE;
        self.history[slot][..self.len].copy_from_slice(&self.buf[..self.len]);
        self.history_lens[slot] = self.len;
        self.history_entries += 1;
    }

    fn load_from_history(&mut self) {
        let slot = (self.history_entries - self.history_offset) % Self::HISTORY_SIZE;
        let len = self.history_lens[slot];
        self.buf[..len].copy_from_slice(&self.history[slot][..len]);
        self.len = len;
        self.cursor = len;
    }

    fn history_up(&mut self) {
        let available = self.history_entries.min(Self::HISTORY_SIZE);
        if self.history_offset >= available {
            return;
        }

        if self.history_offset == 0 {
            // stash current line before replacing it
            self.stashed_buf[..self.len].copy_from_slice(&self.buf[..self.len]);
            self.stashed_len = self.len;
        }

        self.history_offset += 1;
        self.load_from_history();
        self.redraw();
    }

    fn history_down(&mut self) {
        if self.history_offset == 0 {
            return;
        }

        self.history_offset -= 1;

        if self.history_offset == 0 {
            // restore stashed line
            self.buf[..self.stashed_len].copy_from_slice(&self.stashed_buf[..self.stashed_len]);
            self.len = self.stashed_len;
            self.cursor = self.stashed_len;
        } else {
            self.load_from_history();
        }

        self.redraw();
    }
}

impl<'a> Default for LineEditor<'a> {
    fn default() -> Self {
        Self::new()
    }
}

fn write_decimal(buf: &mut [u8], mut n: usize) -> usize {
    let mut tmp = [0u8; 10];
    let mut i = 0;
    if n == 0 {
        buf[i] = b'0';
        return 1;
    }

    while n > 0 {
        tmp[i] = b'0' + (n % 10) as u8;
        i += 1;
        n /= 10;
    }

    tmp[..i].reverse();
    buf[..i].copy_from_slice(&tmp[..i]);
    i
}
