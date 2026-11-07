use kernel::abi::Ioctl;

use crate::fs::{OpenFlag, open};
use crate::io::{Fd, Read, Stderr, Stdin, Stdout, Whence, Write, close, ioctl, seek};

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
    /// file to load history from and append to
    /// None = in-memory only
    history_path: Option<&'a str>,
}

impl<'a> LineEditor<'a> {
    const LINE_MAX: usize = 256;
    const HISTORY_SIZE: usize = 16;
    /// Max bytes of history file read at startup: enough for HISTORY_SIZE full lines + '\n'.
    const HISTORY_WINDOW: usize = Self::HISTORY_SIZE * (Self::LINE_MAX + 1);
    /// Rewrite the file from the ring on load once it grows past this.
    const HISTORY_COMPACT: usize = 4 * Self::HISTORY_WINDOW;

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
            history_path: None,
        }
    }

    pub fn with_history(path: &'a str) -> Self {
        let mut new = Self::new();
        new.history_path = Some(path);
        new.load_history();
        new
    }

    fn history_len(&self) -> usize {
        self.history_entries.min(Self::HISTORY_SIZE)
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
            let line = self.buf;
            if self.push_history(&line[..self.len]) {
                self.append_history(&line[..self.len]);
            }
        }
        self.history_offset = 0;

        Some(unsafe { str::from_utf8_unchecked(&self.buf[..self.len]) })
    }

    /// Called when `0x1b` is read.
    /// Reads the sequence and dispatches the correct handler.
    fn handle_escape(&mut self) {
        let mut seq = [0u8; 8];
        let mut n = 0;

        Stdin.read_exact(&mut seq[..1]).unwrap();
        n += 1;

        match seq[0] {
            b'[' => {
                let mut buf = [0u8; 1];
                loop {
                    Stdin.read_exact(&mut buf).unwrap();

                    if n < seq.len() {
                        seq[n] = buf[0];
                        n += 1;
                    }

                    if (0x40..=0x7E).contains(&buf[0]) {
                        break;
                    }
                }
            }

            b'O' => {
                Stdin.read_exact(&mut seq[n..n + 1]).unwrap();
                n += 1;
            }

            b'f' | b'b' => {}

            _ => return,
        };

        match &seq[..n] {
            b"[A" => self.history_up(),
            b"[B" => self.history_down(),
            b"[C" => self.move_right(),
            b"[D" => self.move_left(),
            b"[1;5C" | b"[1;3C" | b"f" => self.word_right(),
            b"[1;5D" | b"[1;3D" | b"b" => self.word_left(),
            b"[H" | b"OH" | b"[1~" | b"[7~" => self.move_to_start(),
            b"[F" | b"OF" | b"[4~" | b"[8~" => self.move_to_end(),
            b"[3~" => self.delete(),
            _ => {} // unknown, but fully consumed
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

    fn delete(&mut self) {
        if self.cursor == self.len {
            return;
        }

        // shift buf[cursor + 1..len] left by 1
        for i in (self.cursor)..(self.len - 1) {
            self.buf[i] = self.buf[i + 1];
        }

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
        let i = self.prev_word_start(self.cursor);

        // shift buf[cursor..len] left by cursor - i
        for j in self.cursor..self.len {
            self.buf[j - (self.cursor - i)] = self.buf[j];
        }

        self.len -= self.cursor - i;
        self.cursor = i;

        self.redraw();
    }

    fn move_cursor_to(&mut self, to: usize) {
        if self.cursor == to {
            return;
        }

        // ESC [ <digits> D/C = 3 chars + 3 digits
        let mut buf = [0u8; 6];
        buf[0] = b'\x1b';
        buf[1] = b'[';

        let i = if self.cursor < to {
            let delta = to - self.cursor;
            let i = write_decimal(&mut buf[2..5], delta);
            buf[i + 2] = b'C';
            i + 3
        } else {
            let delta = self.cursor - to;
            let i = write_decimal(&mut buf[2..5], delta);
            buf[i + 2] = b'D';
            i + 3
        };

        self.cursor = to;
        Stdout.write_all(&buf[..i]).unwrap();
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.move_cursor_to(self.cursor - 1);
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.len {
            self.move_cursor_to(self.cursor + 1);
        }
    }

    fn prev_word_start(&self, from: usize) -> usize {
        let mut i = from;

        // skip over any spaces before a word
        while i > 0 && self.buf[i - 1] == b' ' {
            i -= 1;
        }

        // skip over the first word
        while i > 0 && self.buf[i - 1] != b' ' {
            i -= 1;
        }

        i
    }

    fn next_word_end(&self, from: usize) -> usize {
        let mut i = from;

        // skip over any spaces before a word
        while i < self.len && self.buf[i] == b' ' {
            i += 1;
        }

        // skip over the first word
        while i < self.len && self.buf[i] != b' ' {
            i += 1;
        }

        i
    }

    fn word_left(&mut self) {
        self.move_cursor_to(self.prev_word_start(self.cursor))
    }

    fn word_right(&mut self) {
        self.move_cursor_to(self.next_word_end(self.cursor))
    }

    fn move_to_start(&mut self) {
        self.move_cursor_to(0);
    }

    fn move_to_end(&mut self) {
        self.move_cursor_to(self.len);
    }

    fn redraw(&self) {
        // Worst case: "\x1b[?25l" (6) + "\r" (1) + prompt + buf (256) + "\x1b[K" (3) + "\x1b[999D" (7) + "\x1b[?25h" (6)
        let mut out = [0u8; 512];
        let mut n = 0;

        // hide cursor
        out[n..n + 6].copy_from_slice(b"\x1b[?25l");
        n += 6;

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

        // show cursor
        out[n..n + 6].copy_from_slice(b"\x1b[?25h");
        n += 6;

        Stdout.write_all(&out[..n]).unwrap();
    }

    fn redraw_full(&self) {
        // \x1b[2J clears the entire screen, \x1b[H moves cursor to top-left
        Stdout.write_all(b"\x1b[2J\x1b[H").unwrap();
        self.redraw();
    }

    /// Adds `line` to the ring unless it's empty or equal to the most recent entry.
    /// Returns whether it was added, so the caller knows whether to persist it.
    fn push_history(&mut self, line: &[u8]) -> bool {
        if line.is_empty() {
            return false;
        }

        if self.history_entries > 0 {
            let prev_slot = (self.history_entries - 1) % Self::HISTORY_SIZE;
            if &self.history[prev_slot][..self.history_lens[prev_slot]] == line {
                return false;
            }
        }

        let slot = self.history_entries % Self::HISTORY_SIZE;
        self.history[slot][..line.len()].copy_from_slice(line);
        self.history_lens[slot] = line.len();
        self.history_entries += 1;

        true
    }

    /// Reads the tail of the history file into the ring; compacts the file if it's large.
    fn load_history(&mut self) {
        let Some(path) = self.history_path else {
            return;
        };

        let Ok(mut fd) = open(path, OpenFlag::READ_ONLY) else {
            return;
        };

        let Ok(size) = seek(fd, 0, Whence::End) else {
            let _ = close(fd);
            return;
        };

        let start = size.saturating_sub(Self::HISTORY_WINDOW);
        if seek(fd, start as isize, Whence::Start).is_err() {
            let _ = close(fd);
            return;
        };

        let len = size - start;
        let mut buf = [0u8; Self::HISTORY_WINDOW];
        if fd.read_exact(&mut buf[..len]).is_err() {
            let _ = close(fd);
            return;
        }

        buf[..len]
            .split(|&c| c == b'\n')
            // skip past the first '\n' because we might have read mid line
            .skip(if start > 0 { 1 } else { 0 })
            .filter(|&l| !l.is_empty() && l.len() <= Self::LINE_MAX)
            .for_each(|l| {
                self.push_history(l);
            });

        let _ = close(fd);

        if size > Self::HISTORY_COMPACT {
            self.rewrite_history();
        }
    }

    /// Appends `line` + '\n' to the history file in one write.
    fn append_history(&self, line: &[u8]) {
        let Some(path) = self.history_path else {
            return;
        };

        let mut buf = [0u8; Self::LINE_MAX + 1];
        buf[..line.len()].copy_from_slice(line);
        buf[line.len()] = b'\n';

        let Ok(mut fd) = open(
            path,
            OpenFlag::WRITE_ONLY | OpenFlag::CREATE | OpenFlag::APPEND,
        ) else {
            return;
        };

        let _ = fd.write_all(&buf[..line.len() + 1]);
        let _ = close(fd);
    }

    /// Replaces the file with the current ring, oldest first.
    fn rewrite_history(&self) {
        let Some(path) = self.history_path else {
            return;
        };

        let Ok(mut fd) = open(
            path,
            OpenFlag::WRITE_ONLY | OpenFlag::CREATE | OpenFlag::TRUNCATE,
        ) else {
            return;
        };

        let mut buf = [0u8; Self::LINE_MAX + 1];
        let available = self.history_len();
        for k in 0..available {
            let slot = (self.history_entries - available + k) % Self::HISTORY_SIZE;
            let line = &self.history[slot][..self.history_lens[slot]];
            buf[..line.len()].copy_from_slice(line);
            buf[line.len()] = b'\n';
            let _ = fd.write_all(&buf[..line.len() + 1]);
        }

        let _ = close(fd);
    }

    fn load_from_history(&mut self) {
        let slot = (self.history_entries - self.history_offset) % Self::HISTORY_SIZE;
        let len = self.history_lens[slot];
        self.buf[..len].copy_from_slice(&self.history[slot][..len]);
        self.len = len;
        self.cursor = len;
    }

    fn history_up(&mut self) {
        let available = self.history_len();
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
