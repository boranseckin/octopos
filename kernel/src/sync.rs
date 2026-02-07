use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;

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

    unsafe fn get_unchecked(&self) -> &T {
        unsafe { (*self.value.get()).assume_init_ref() }
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
