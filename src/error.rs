#[repr(isize)]
#[derive(Debug)]
pub enum KernelError {
    AllocError = -1,
    InvalidPageError = -2,
}

impl From<core::alloc::AllocError> for KernelError {
    fn from(value: core::alloc::AllocError) -> Self {
        Self::AllocError
    }
}

impl KernelError {
    pub fn as_str(&self) -> &'static str {
        match self {
            KernelError::AllocError => "alloc error",
            KernelError::InvalidPageError => "invalid page",
        }
    }
}

impl core::fmt::Display for KernelError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}
