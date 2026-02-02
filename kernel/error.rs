use core::fmt::Display;

use crate::exec::ExecError;
use crate::fs::FsError;
use crate::syscall::SyscallError;
use crate::virtio_disk::VirtioError;
use crate::vm::VmError;

/// Kernel error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelError {
    Alloc,
    InvalidArgument,
    OutOfProc,
    Vm(VmError),
    Syscall(SyscallError),
    Fs(FsError),
    Exec(ExecError),
    VirtioError(VirtioError),
}

impl From<core::alloc::AllocError> for KernelError {
    fn from(_value: core::alloc::AllocError) -> Self {
        Self::Alloc
    }
}

impl From<VmError> for KernelError {
    fn from(value: VmError) -> Self {
        Self::Vm(value)
    }
}

impl From<SyscallError> for KernelError {
    fn from(value: SyscallError) -> Self {
        Self::Syscall(value)
    }
}

impl From<FsError> for KernelError {
    fn from(value: FsError) -> Self {
        Self::Fs(value)
    }
}

impl From<ExecError> for KernelError {
    fn from(value: ExecError) -> Self {
        Self::Exec(value)
    }
}

impl From<VirtioError> for KernelError {
    fn from(value: VirtioError) -> Self {
        Self::VirtioError(value)
    }
}

impl Display for KernelError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            KernelError::Alloc => write!(f, "alloc error"),
            KernelError::InvalidArgument => write!(f, "invalid argument"),
            KernelError::OutOfProc => write!(f, "out of proc"),
            KernelError::Syscall(e) => write!(f, "syscall error: {}", e),
            KernelError::Vm(e) => write!(f, "vm error: {}", e),
            KernelError::Fs(e) => write!(f, "filesystem error {}", e),
            KernelError::Exec(e) => write!(f, "exec error {}", e),
            KernelError::VirtioError(e) => write!(f, "virtio error {}", e),
        }
    }
}

/// Return an error, logging file:line. Use instead of `return Err(...)`.
#[macro_export]
macro_rules! err {
    ($e:expr) => {{
        #[cfg(debug_assertions)]
        {
            let _lock = $crate::proc::CPU_POOL.lock_current();
            #[allow(unused_unsafe)]
            let cpu_id = unsafe { $crate::proc::CPU_POOL.current_id() };
            $crate::println!(
                "! hart {} errored at {}:{}: {}",
                cpu_id,
                file!(),
                line!(),
                $e
            );
        }
        return Err($e.into());
    }};
}

/// Log error.
#[macro_export]
macro_rules! log {
    ($e:expr) => {
        match $e {
            Ok(v) => Ok(v),
            Err(e) => {
                #[cfg(debug_assertions)]
                $crate::println!("  at {}:{}", file!(), line!());
                Err(e)
            }
        }
    };
}

/// Propagate error with location logging. Use instead of `?`.
#[macro_export]
macro_rules! try_log {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(e) => {
                #[cfg(debug_assertions)]
                $crate::println!("  at {}:{}", file!(), line!());
                return Err(e.into());
            }
        }
    };
}
