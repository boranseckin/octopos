use core::cell::{Cell, UnsafeCell};
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ops::Deref;

use crate::spinlock::SpinLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnceLockState {
    Incomplete,
    Complete,
}

/// A synchronization primitive which can be initialized exactly once.
#[derive(Debug)]
pub struct OnceLock<T> {
    state: SpinLock<OnceLockState>,
    value: UnsafeCell<MaybeUninit<T>>,
    _marker: PhantomData<T>,
}

impl<T> OnceLock<T> {
    pub const fn new() -> Self {
        Self {
            state: SpinLock::new(OnceLockState::Incomplete, "oncecell"),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            _marker: PhantomData,
        }
    }

    fn is_init(&self) -> bool {
        *self.state.lock() == OnceLockState::Complete
    }

    pub fn initialize<F, E>(&self, f: F)
    where
        F: FnOnce() -> Result<T, E>,
    {
        let mut state = self.state.lock();

        // if incomplete, initialize.
        // otherwise, another thread must have initialized it, do nothing.
        if *state == OnceLockState::Incomplete {
            match f() {
                Ok(value) => {
                    unsafe { (*self.value.get()).write(value) };
                    *state = OnceLockState::Complete;
                }
                Err(_e) => panic!("failed to init once lock"),
            }
        }
    }

    pub fn get(&self) -> Option<&T> {
        if self.is_init() {
            Some(unsafe { self.get_unchecked() })
        } else {
            None
        }
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        if self.is_init() {
            Some(unsafe { self.get_unchecked_mut() })
        } else {
            None
        }
    }

    pub fn get_or_init<F>(&self, f: F) -> &T
    where
        F: FnOnce() -> T,
    {
        // between `get` and `initialize`, the lock is released,
        // so another thread may have initialized it.
        match self.get() {
            Some(value) => value,
            None => {
                self.initialize(|| Ok::<T, ()>(f()));
                unsafe { self.get_unchecked() }
            }
        }
    }

    unsafe fn get_unchecked(&self) -> &T {
        unsafe { (*self.value.get()).assume_init_ref() }
    }

    unsafe fn get_unchecked_mut(&mut self) -> &mut T {
        unsafe { (*self.value.get()).assume_init_mut() }
    }
}

impl<T> Drop for OnceLock<T> {
    fn drop(&mut self) {
        if self.is_init() {
            unsafe { self.value.get_mut().assume_init_drop() }
        }
    }
}

impl<T> Default for OnceLock<T> {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl<T: Sync + Send> Sync for OnceLock<T> {}
unsafe impl<T: Send> Send for OnceLock<T> {}

/// A lazyly initialized value.
///
/// The value is initialized on the first access using the provided function.
/// This implementation is prone to poisoning if the initialization function panics.
pub struct LazyLock<T, F = fn() -> T> {
    once: OnceLock<T>,
    // Use option since F does not have a default after Cell::take()
    func: Cell<Option<F>>,
}

impl<T, F> LazyLock<T, F> {
    pub const fn new(f: F) -> Self {
        Self {
            once: OnceLock::new(),
            func: Cell::new(Some(f)),
        }
    }
}

impl<T, F: FnOnce() -> T> LazyLock<T, F> {
    pub fn force(this: &LazyLock<T, F>) -> &T {
        this.once.get_or_init(|| {
            let f = this.func.take().expect("lazy lock to be not init");
            f()
        })
    }
}

impl<T, F: FnOnce() -> T> Deref for LazyLock<T, F> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        LazyLock::force(self)
    }
}

unsafe impl<T: Sync + Send, F: Send> Sync for LazyLock<T, F> {}
