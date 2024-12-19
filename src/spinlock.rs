use core::cell::UnsafeCell;
use core::hint;
use core::ops::{Deref, DerefMut};
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use crate::proc::InterruptLock;
use crate::proc::{Cpu, Cpus};
use crate::riscv::interrupts;

pub struct SpinLock {
    pub name: &'static str,
    pub locked: AtomicBool,
    pub cpu: *mut Cpu,
}

unsafe impl Sync for SpinLock {}

impl SpinLock {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            locked: AtomicBool::new(false),
            cpu: ptr::null_mut(),
        }
    }

    pub fn acquire(&mut self) {
        push_off();

        unsafe {
            assert!(!holding(self), "acquire {}", self.name);

            loop {
                if self
                    .locked
                    .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    self.cpu = Cpus::mycpu();
                    break;
                }

                hint::spin_loop()
            }
        }
    }

    pub fn release(&mut self) {
        unsafe {
            assert!(holding(self), "release {}", self.name);

            self.cpu = ptr::null_mut();
            self.locked.store(false, Ordering::Relaxed);

            pop_off();
        }
    }
}

unsafe fn holding(lock: &SpinLock) -> bool {
    lock.locked.load(Ordering::Relaxed) && lock.cpu == unsafe { Cpus::mycpu() }
}

pub fn push_off() {
    let old = interrupts::get();
    interrupts::disable();
    unsafe {
        let c = &mut *Cpus::mycpu();
        if c.num_off == 0 {
            c.interrupt_enabled = old;
        }
        c.num_off += 1;
    }
}

pub fn pop_off() {
    assert!(!interrupts::get(), "pop_off - interruptable");

    unsafe {
        let c = &mut *Cpus::mycpu();
        assert!(c.num_off >= 1, "pop_off");

        c.num_off -= 1;
        if c.num_off == 0 && c.interrupt_enabled {
            interrupts::enable();
        }
    }
}

// Locked when CPU pointer is not null.
#[derive(Debug)]
pub struct Mutex<T> {
    name: &'static str,
    cpu: AtomicPtr<Cpu>,
    data: UnsafeCell<T>,
}

// Safety: UnsafeCell is not Sync but it can only be consumed with a guard
// or an exclusive reference. So Mutex is safe to sync, if the inner type T is.
unsafe impl<T> Sync for Mutex<T> where T: Send {}

pub struct MutexGuard<'a, T: 'a> {
    mutex: &'a Mutex<T>,
    _intr_lock: InterruptLock,
}

// Safety: UnsafeCell inside Mutex is not Sync but only one thread can hold this guard.
// So MutexGuard is safe to sync as long as the inner type T is.
unsafe impl<T> Sync for MutexGuard<'_, T> where T: Sync {}

impl<T> Mutex<T> {
    pub const fn new(value: T, name: &'static str) -> Self {
        Mutex {
            name,
            cpu: AtomicPtr::new(ptr::null_mut()),
            data: UnsafeCell::new(value),
        }
    }

    // Safety: must be called with interrupts disabled.
    unsafe fn holding(&self) -> bool {
        self.cpu.load(Ordering::Relaxed) == unsafe { Cpus::mycpu() }
    }

    pub fn lock(&self) -> MutexGuard<T> {
        let _intr_lock = Cpus::lock_mycpu();

        unsafe {
            assert!(!self.holding(), "acquire lock {}", self.name);

            loop {
                if self
                    .cpu
                    .compare_exchange(
                        ptr::null_mut(),
                        Cpus::mycpu(),
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    break MutexGuard {
                        mutex: self,
                        _intr_lock,
                    };
                }

                hint::spin_loop()
            }
        }
    }

    // Since this call consumes self, we can guarentee no one else is holding a reference.
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }

    // Since this call mutably borrows self, we can guarentee no one else is holding a reference.
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    // Use this over `get_mut` when you need unsafe mutable access.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn get_mut_unchecked(&self) -> &mut T {
        &mut *self.data.get()
    }
}

// Dropping the guard will release the lock on the mutex and also release the interrupt lock.
impl<'a, T: 'a> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        // Safety: mutex guard has an interrupt lock, it is safe to call holding
        unsafe {
            assert!(self.mutex.holding(), "release lock {}", self.mutex.name);
        }

        self.mutex.cpu.store(ptr::null_mut(), Ordering::Release);
    }
}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.mutex.data.get() }
    }
}
