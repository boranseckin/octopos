use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;

use crate::spinlock::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnceLockState {
    Incomplete,
    Complete,
}

#[derive(Debug)]
pub struct OnceLock<T> {
    state: Mutex<OnceLockState>,
    value: UnsafeCell<MaybeUninit<T>>,
    _marker: PhantomData<T>,
}

impl<T> OnceLock<T> {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(OnceLockState::Incomplete, "oncecell"),
            value: UnsafeCell::new(MaybeUninit::uninit()),
            _marker: PhantomData,
        }
    }

    fn is_init(&self) -> bool {
        // deref mutex_guard to access inner
        *self.state.lock() == OnceLockState::Complete
    }

    pub fn initialize<F, E>(&self, f: F) -> Result<(), E>
    where
        F: FnOnce() -> Result<T, E>,
    {
        let mut state = self.state.lock();
        if *state == OnceLockState::Incomplete {
            match f() {
                Ok(value) => {
                    unsafe { (*self.value.get()).write(value) };
                    *state = OnceLockState::Complete;
                    Ok(())
                }
                Err(e) => Err(e),
            }
        } else {
            panic!("double init sync lock");
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

unsafe impl<T: Sync + Send> Sync for OnceLock<T> {}
unsafe impl<T: Send> Send for OnceLock<T> {}
