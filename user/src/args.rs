use core::arch::asm;
use core::slice;
use core::str::Utf8Error;

/// Represents command-line arguments passed to the program.
pub struct Args {
    argc: usize,
    argv: *const *const u8,
}

/// Iterator over command-line arguments, skipping the program name.
pub struct ArgsIter {
    argv: *const *const u8,
    current: usize,
    end: usize,
}

pub struct ArgsStrIter {
    iter: ArgsIter,
}

impl Args {
    /// Constructs `Args` from the stack.
    ///
    /// # Safety
    /// Must be called at program start, before any other function calls.
    #[inline(always)]
    pub unsafe fn from_stack() -> Self {
        let argc: usize;
        let argv: *const *const u8;

        unsafe {
            asm!(
                "mv {0}, a0", // argc from a0 (exec return value)
                "mv {1}, a1", // argv from a1
                out(reg) argc,
                out(reg) argv,
            )
        };

        Self { argc, argv }
    }

    /// Returns the number of command-line arguments, including the program name.
    #[allow(clippy::len_without_is_empty)] // empty arg is not possible
    pub fn len(&self) -> usize {
        self.argc
    }

    /// Returns the number of command-line arguments, excluding the program name.
    pub fn args_len(&self) -> usize {
        self.argc.saturating_sub(1)
    }

    /// Gets the program name as a byte slice.
    pub fn program(&self) -> Option<&'static [u8]> {
        self.get(0)
    }

    /// Gets the argument at the specified index as a byte slice.
    pub fn get(&self, index: usize) -> Option<&'static [u8]> {
        if index >= self.argc {
            return None;
        }

        unsafe {
            let ptr = *self.argv.add(index);
            let mut len = 0;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            Some(slice::from_raw_parts(ptr, len))
        }
    }

    /// Iterates args, including the program name.
    pub fn iter(&self) -> ArgsIter {
        ArgsIter {
            argv: self.argv,
            current: 0,
            end: self.argc,
        }
    }

    /// Iterates args as `&str`, including the program name.
    pub fn iter_as_str(&self) -> ArgsStrIter {
        ArgsStrIter { iter: self.iter() }
    }

    /// Iterates args, excluding the program name.
    pub fn args(&self) -> ArgsIter {
        ArgsIter {
            argv: self.argv,
            current: 1,
            end: self.argc,
        }
    }

    /// Iterates args as `&str`, excluding the program name.
    pub fn args_as_str(&self) -> ArgsStrIter {
        ArgsStrIter { iter: self.args() }
    }
}

impl Iterator for ArgsIter {
    type Item = &'static [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.current >= self.end {
            return None;
        }

        unsafe {
            let ptr = *self.argv.add(self.current);
            self.current += 1;

            let mut len = 0;
            while *ptr.add(len) != 0 {
                len += 1;
            }

            Some(slice::from_raw_parts(ptr, len))
        }
    }
}

impl Iterator for ArgsStrIter {
    type Item = &'static str;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().and_then(|arg| str::from_utf8(arg).ok())
    }
}

impl IntoIterator for &Args {
    type Item = &'static [u8];

    type IntoIter = ArgsIter;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Converts a C-style string pointer to a Rust-style `&str`.
///
/// # Safety
/// The caller must ensure that `cstr` is a valid null-terminated UTF-8 string.
pub unsafe fn str_from_cstr<'a>(cstr: &[u8]) -> Result<&'a str, Utf8Error> {
    let ptr = cstr.as_ptr();
    unsafe {
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        str::from_utf8(slice::from_raw_parts(ptr, len))
    }
}
