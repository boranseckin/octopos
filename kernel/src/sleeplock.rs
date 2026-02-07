use core::cell::UnsafeCell;

use crate::proc::{self, Channel, Pid};
use crate::spinlock::SpinLock;

/// Inner state of a SleepLock.
/// This is guarded by a SpinLock.
#[derive(Debug)]
pub struct SleepLockInner {
    locked: bool,
    pid: Option<Pid>,
}

/// A lock that causes the caller to sleep while waiting.
/// Unlike SpinLock, interrupts remain enabled while holding a SleepLock.
#[derive(Debug)]
pub struct SleepLock<T> {
    _name: &'static str,
    /// SpinLock only protects the lock state and not the data
    inner: SpinLock<SleepLockInner>,
    data: UnsafeCell<T>,
}

/// A guard that releases the SleepLock when dropped.
#[derive(Debug)]
pub struct SleepLockGuard<'a, T: 'a> {
    lock: &'a SleepLock<T>,
}

impl<T> SleepLock<T> {
    pub const fn new(value: T, name: &'static str) -> Self {
        SleepLock {
            _name: name,
            inner: SpinLock::new(
                SleepLockInner {
                    pid: None,
                    locked: false,
                },
                name,
            ),
            data: UnsafeCell::new(value),
        }
    }

    /// Returns true if the current process is holding the lock.
    pub fn holding(&self) -> bool {
        let inner = self.inner.lock();

        inner.locked && (inner.pid == Some(proc::current_proc().inner.lock().pid))
    }

    /// Acquires the mutex without disabling interrupts or blocking the current thread.
    pub fn lock(&self) -> SleepLockGuard<'_, T> {
        let mut inner = self.inner.lock();

        while inner.locked {
            inner = proc::sleep(Channel::Lock(self as *const _ as usize), inner);
        }

        inner.locked = true;
        inner.pid = Some(proc::current_proc().inner.lock().pid);

        SleepLockGuard { lock: self }
    }

    /// Consumes the mutex and returns the inner data.
    pub fn into_inner(self) -> T {
        self.data.into_inner()
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

impl<'a, T: 'a> Drop for SleepLockGuard<'a, T> {
    fn drop(&mut self) {
        let mut inner = self.lock.inner.lock();
        inner.locked = false;
        inner.pid = None;

        // wake up any waiters before dropping the spinlock
        proc::wakeup(Channel::Lock(self.lock as *const _ as usize));
    }
}

impl<T> core::ops::Deref for SleepLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> core::ops::DerefMut for SleepLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

/// # Safety
/// The lock can give `&mut T` to whichever thread acquires it and can call `into_inner()`.
/// Therefore, `T` must be `Send` to ensure that it is safe to send the inner data across threads.
unsafe impl<T> Sync for SleepLock<T> where T: Send {}

/// # Safety
/// `Send`ing the lock also transfers the ownership of the inner data `T`.
/// Therefore, `T` must be `Send` to ensure that it is safe to send the inner data across threads.
unsafe impl<T> Send for SleepLock<T> where T: Send {}

// Note: `SleepLockGuard` is intentionally `!Sync` (no `Sync` impl). Unlike `SpinLockGuard`, a
// sleep lock guard is always held by a single sleeping process and never shared across threads, so
// there is no need to allow `&SleepLockGuard` to cross thread boundaries.
