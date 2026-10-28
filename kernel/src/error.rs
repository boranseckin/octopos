use core::fmt::{self, Display};
use core::ops::Deref;
#[cfg(debug_assertions)]
use core::panic::Location;

use crate::param::TRACK_EXPECTED_ERRORS;

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
/// Requires an existing `From<$a> from $b`.
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

/// Errors that are normal control flow and are not reported.
pub trait Expected {
    fn is_expected(&self) -> bool {
        false
    }
}

pub fn report<E: Display + Expected>(e: &Traced<E>, ctx: fmt::Arguments) {
    if !cfg!(debug_assertions) || (e.is_expected() && !TRACK_EXPECTED_ERRORS) {
        return;
    }

    let _lock = crate::proc::lock_current_cpu();
    let cpu_id = unsafe { crate::proc::current_id() };
    crate::println!("! hart {} {}: {}", cpu_id, ctx, e);

    #[cfg(debug_assertions)]
    {
        for t in e.trail() {
            crate::println!("  at {}:{}", t.file(), t.line());
        }

        if e.trail.dropped > 0 {
            crate::println!("  (+{} more)", e.trail.dropped);
        }
    }

    crate::println!("");
}

/// Top-of-stack sink: prints `! hart N <context>: <kind>` and the trail, unless the kind is
/// expected. No-op in release.
#[macro_export]
macro_rules! report {
    ($e:expr, $($ctx:tt)*) => {
        if cfg!(debug_assertions) {
            $crate::error::report(&$e, format_args!($($ctx)*))
        }
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

/// Returns early with `$e` unless `$cond` holds.
#[macro_export]
macro_rules! ensure {
    ($cond:expr, $e:expr) => {
        if !$cond {
            $crate::err!($e)
        }
    };
}
