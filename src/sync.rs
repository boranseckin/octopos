use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

use crate::spinlock::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnceLockState {
    Incomplete,
    Complete,
}

pub struct OnceCell<T> {
    state: Mutex<OnceLockState>,
    value: UnsafeCell<MaybeUninit<T>>,
}

impl<T> OnceCell<T> {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(OnceLockState::Incomplete, "oncecell"),
            value: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    fn is_init(&self) -> bool {
        *self.state.lock() == OnceLockState::Complete
    }

    pub fn initialize<F>(&self, f: F)
    where
        F: FnOnce() -> T,
    {
        let mut state = self.state.lock();
        if *state == OnceLockState::Incomplete {
            unsafe { (*self.value.get()).write(f()) };
            *state = OnceLockState::Complete;
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

    pub unsafe fn get_unchecked(&self) -> &T {
        unsafe { (*self.value.get()).assume_init_ref() }
    }

    pub unsafe fn get_unchecked_mut(&mut self) -> &mut T {
        unsafe { (*self.value.get()).assume_init_mut() }
    }
}

impl<T> Drop for OnceCell<T> {
    fn drop(&mut self) {
        if self.is_init() {
            unsafe { self.value.get_mut().assume_init_drop() }
        }
    }
}

unsafe impl<T: Sync + Send> Sync for OnceCell<T> {}
unsafe impl<T: Send> Send for OnceCell<T> {}
