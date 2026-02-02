use core::cell::UnsafeCell;
use core::hint;
use core::ops::{Deref, DerefMut};
use core::ptr;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::proc::{CPU, CPU_POOL, InterruptLock};

/// A mutual exclusion primitive useful for protecting shared data.
/// It uses a spinlock to achieve mutual exclusion.
#[derive(Debug)]
pub struct SpinLock<T> {
    name: &'static str,
    cpu: AtomicPtr<CPU>,
    data: UnsafeCell<T>,
}

/// A guard that releases the lock when dropped.
pub struct SpinLockGuard<'a, T: 'a> {
    lock: &'a SpinLock<T>,
    _intr_lock: InterruptLock,
}

impl<T> SpinLock<T> {
    pub const fn new(value: T, name: &'static str) -> Self {
        SpinLock {
            name,
            cpu: AtomicPtr::new(ptr::null_mut()),
            data: UnsafeCell::new(value),
        }
    }

    /// Returns true if the current CPU is holding the lock.
    /// # Safety: must be called with interrupts disabled.
    unsafe fn holding(&self) -> bool {
        self.cpu.load(Ordering::Relaxed) == unsafe { CPU_POOL.current() }
    }

    /// Acquires the mutex, blocking the current thread until it is able to do so.
    ///
    /// Returns a guard that releases the lock when dropped.
    ///
    /// Current thread's interrupts will be disabled while holding the lock.
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        let intr_lock = CPU_POOL.lock_current();

        // Safety: interrupts are disabled
        unsafe {
            assert!(!self.holding(), "acquire spinlock {}", self.name);
        }

        loop {
            if self
                .cpu
                .compare_exchange(
                    ptr::null_mut(),
                    // Safety: interrupts are disabled
                    unsafe { CPU_POOL.current() },
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
                .is_ok()
            {
                break SpinLockGuard {
                    lock: self,
                    _intr_lock: intr_lock,
                };
            }

            hint::spin_loop()
        }
    }

    /// Releases the lock on the mutex.
    ///
    /// Interrupt lock held by the guard will also be released, restoring the previous interrupt
    /// state.
    pub fn unlock(guard: SpinLockGuard<'_, T>) -> &'_ SpinLock<T> {
        guard.lock
    }

    /// Unlocks the mutex without a guard and manually releases the `InterruptLock`.
    ///
    /// # Safety
    /// Used by `fork_ret` to unlock after returning from scheduler.
    pub unsafe fn force_unlock(&self) {
        unsafe {
            assert!(self.holding(), "force_unlock: not locked {}", self.name);
            self.cpu.store(ptr::null_mut(), Ordering::Release);
            // also release interrupt lock (decrement num_off)
            CPU_POOL.current().unlock();
        }
    }

    /// Consumes the mutex and returns the inner data.
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }

    /// Returns a mutable reference to the inner data.
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    /// Returns a reference to the inner data from a shared reference to the mutex.
    ///
    /// # Safety
    /// The caller must ensure that the mutex is locked.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut_unchecked(&self) -> &mut T {
        unsafe { &mut *self.data.get() }
    }
}

/// Dropping the guard will release the lock on the mutex and also release the interrupt lock.
impl<'a, T: 'a> Drop for SpinLockGuard<'a, T> {
    fn drop(&mut self) {
        assert!(
            // Safety: mutex guard has an interrupt lock, it is safe to call holding
            unsafe { self.lock.holding() },
            "release lock {}",
            self.lock.name
        );

        self.lock.cpu.store(ptr::null_mut(), Ordering::Release);
    }
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

// Safety: Since the holder can call `into_inner`, if we are sharing a reference, the inner type
// must also be thread safe to Send.
unsafe impl<T> Sync for SpinLock<T> where T: Send {}

// Safety: SpinLock can be sent to another thread if T can be sent.
unsafe impl<T> Send for SpinLock<T> where T: Send {}

// Safety: Since the holder can call `Deref`, if we are sharing a reference, the inner type must
// also be thread safe to Sync.
unsafe impl<T> Sync for SpinLockGuard<'_, T> where T: Sync {}
