use core::fmt::{self, Display};
use core::ops::Deref;
#[cfg(debug_assertions)]
use core::panic::Location;

use crate::exec::ExecError;
use crate::fs::FsError;
use crate::net::NetError;
use crate::syscall::SysError;
use crate::virtio_disk::VirtioError;
use crate::vm::VmError;

#[cfg(debug_assertions)]
const TRAIL: usize = 4;

#[derive(Debug, Clone, Copy)]
pub struct Traced<E> {
    pub kind: E,
    #[cfg(debug_assertions)]
    trail: Trail,
}

#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy)]
struct Trail {
    at: [Option<&'static Location<'static>>; TRAIL],
    dropped: u8,
}

#[cfg(debug_assertions)]
impl Trail {
    fn new(origin: &'static Location<'static>) -> Self {
        Self {
            at: {
                let mut a = [None; TRAIL];
                a[0] = Some(origin);
                a
            },
            dropped: 0,
        }
    }

    /// Appends `loc`, or increases `dropped` when full.
    fn push(&mut self, loc: &'static Location<'static>) {
        if let Some(at) = self.at.iter_mut().find(|l| l.is_none()) {
            at.replace(loc);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}

impl<E> Traced<E> {
    /// Changes the kind, keeping the trail and appending the caller.
    #[track_caller]
    pub fn map_kind<F>(self, f: impl FnOnce(E) -> F) -> Traced<F> {
        #[cfg_attr(not(debug_assertions), allow(unused_mut))]
        let mut new = Traced {
            kind: f(self.kind),
            #[cfg(debug_assertions)]
            trail: self.trail,
        };
        #[cfg(debug_assertions)]
        new.trail.push(Location::caller());
        new
    }

    #[cfg(debug_assertions)]
    pub fn trail(&self) -> impl Iterator<Item = &'static Location<'static>> {
        self.trail.at.into_iter().flatten()
    }
}

impl<E> Deref for Traced<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.kind
    }
}

/// Origin of most traced errors. `?` on a raw error kind lands here and gets converted while
/// `#[track_caller]` records the `?` site.
impl<E> From<E> for Traced<E> {
    #[track_caller]
    fn from(value: E) -> Self {
        Self {
            kind: value,
            #[cfg(debug_assertions)]
            trail: Trail::new(Location::caller()),
        }
    }
}

/// Prints only the kind. The trail is printed by `report!`.
impl<E: Display> Display for Traced<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)
    }
}

/// Crossing between error types.
/// Requires an existing `From<$a> fro $b`.
macro_rules! traced_from {
    ($a:ty => $b:ty) => {
        // Traced<A> -> Traced<B>: convert kind, keep trail, push caller.
        impl From<$crate::error::Traced<$a>> for $crate::error::Traced<$b> {
            #[track_caller]
            fn from(e: $crate::error::Traced<$a>) -> Self {
                e.map_kind(<$b>::from)
            }
        }

        // raw A -> Traced<B>
        impl From<$a> for $crate::error::Traced<$b> {
            #[track_caller]
            fn from(e: $a) -> Self {
                Self::from(<$b>::from(e))
            }
        }
    };
}

/// Kernel error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelError {
    Alloc,
    InvalidArgument,
    OutOfProc,
    Vm(VmError),
    Sys(SysError),
    Fs(FsError),
    Exec(ExecError),
    VirtioError(VirtioError),
    Net(NetError),
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

impl From<SysError> for KernelError {
    fn from(value: SysError) -> Self {
        Self::Sys(value)
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

impl From<NetError> for KernelError {
    fn from(value: NetError) -> Self {
        Self::Net(value)
    }
}

impl Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KernelError::Alloc => write!(f, "alloc error"),
            KernelError::InvalidArgument => write!(f, "invalid argument"),
            KernelError::OutOfProc => write!(f, "out of proc"),
            KernelError::Sys(e) => write!(f, "syscall error {}", e),
            KernelError::Vm(e) => write!(f, "vm error {}", e),
            KernelError::Fs(e) => write!(f, "filesystem error {}", e),
            KernelError::Exec(e) => write!(f, "exec error {}", e),
            KernelError::VirtioError(e) => write!(f, "virtio error {}", e),
            KernelError::Net(e) => write!(f, "net error {e}"),
        }
    }
}

/// Errors that are normal control flow and are not traced.
pub trait Expected {
    fn is_expected(&self) -> bool {
        false
    }
}

#[cfg_attr(not(debug_assertions), allow(unused_variables))]
pub fn report<E: Display + Expected>(e: &Traced<E>, ctx: fmt::Arguments) {
    if !e.is_expected() {
        #[cfg(debug_assertions)]
        {
            let _lock = crate::proc::lock_current_cpu();
            let cpu_id = unsafe { crate::proc::current_id() };
            crate::println!("! hart {} {}: {}", cpu_id, ctx, e);

            for t in e.trail() {
                println!("  at {}:{}", t.file(), t.line());
            }

            println!("");
        }
    }
}

/// Top-of-stack sink: prints `! hart N <context>: <kind>` and the trail, unless the kind is
/// expected. No-op in release.
macro_rules! report {
    ($e:expr, $($ctx:tt)*) => {
        $crate::error::report(&$e, format_args!($($ctx)*))
    };
}

/// Returns early with an error, recording this line as the origin.
#[macro_export]
macro_rules! err {
    // This builds a `Result<Infallible, E>`. Since `Infallible` can never happen, it always becomes
    // an `Err`. `?` tries to convert the `$e` into the function's error type using `From::from` and
    // returns early.
    ($e:expr) => {{ match Err::<core::convert::Infallible, _>($e)? {} }};
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
