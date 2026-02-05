use core::slice;

pub struct Args {
    argc: usize,
    argv: *const *const u8,
}

impl Args {
    /// # Safety
    /// Must be called at program start, before any other function calls.
    #[inline(always)]
    pub unsafe fn from_stack() -> Self {
        let argc: usize;
        let argv: *const *const u8;

        unsafe {
            core::arch::asm!(
            "mv {0}, a0", // argc from a0 (exec return value)
            "mv {1}, a1", // argv from a1
            out(reg) argc,
            out(reg) argv,
            )
        };

        Self { argc, argv }
    }

    pub fn len(&self) -> usize {
        self.argc
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

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
}

/// Converts a C-style string pointer to a Rust-style `&str`.
///
/// # Safety
/// The caller must ensure that `ptr` is a valid pointer to a null-terminated UTF-8 string.
pub unsafe fn str_from_cstr<'a>(ptr: *const u8) -> &'a str {
    unsafe {
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        str::from_utf8_unchecked(slice::from_raw_parts(ptr, len))
    }
}
