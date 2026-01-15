/// Kernel error codes.
#[repr(isize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelError {
    Alloc = -1,
    InvalidPage = -2,
    InvalidAddress = -3,
    InvalidPte = -4,
}

impl From<core::alloc::AllocError> for KernelError {
    fn from(_value: core::alloc::AllocError) -> Self {
        Self::Alloc
    }
}

impl KernelError {
    pub fn as_str(&self) -> &'static str {
        match self {
            KernelError::Alloc => "alloc error",
            KernelError::InvalidPage => "invalid page",
            KernelError::InvalidAddress => "invalid address",
            KernelError::InvalidPte => "invalid pte",
        }
    }
}

impl core::fmt::Display for KernelError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}
