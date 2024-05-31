use alloc::sync::Arc;
use core::cell::UnsafeCell;
use core::mem::{transmute, MaybeUninit};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::param::NCPU;
use crate::riscv::interrupts;
use crate::riscv::registers::tp;
use crate::spinlock::SpinLock;

pub static CPUS: Cpus = Cpus::new();

pub struct Cpus([UnsafeCell<Cpu>; NCPU]);
unsafe impl Sync for Cpus {}

pub struct Cpu {
    pub proc: Option<Arc<Proc>>,
    pub context: Context,
    pub num_off: isize,
    pub interrupt_enabled: bool,
}

impl Cpu {
    const fn new() -> Self {
        Self {
            proc: None,
            context: Context::new(),
            num_off: 0,
            interrupt_enabled: false,
        }
    }

    fn lock(&mut self, old_state: bool) -> InterruptLock {
        if self.num_off == 0 {
            self.interrupt_enabled = old_state;
        }
        self.num_off += 1;
        InterruptLock {}
    }

    fn unlock(&mut self) {
        assert!(!interrupts::get(), "cpu unlock - interruptible");
        assert!(self.num_off >= 1, "cpu unlock");

        self.num_off -= 1;
        if self.num_off == 0 && self.interrupt_enabled {
            interrupts::enable();
        }
    }
}

impl Cpus {
    const fn new() -> Self {
        let mut array: [MaybeUninit<_>; NCPU] = unsafe { MaybeUninit::uninit().assume_init() };
        let mut i = 0;
        while i < NCPU {
            array[i] = MaybeUninit::new(UnsafeCell::new(Cpu::new()));
            i += 1;
        }
        unsafe { transmute(array) }
    }

    /// Return the hart id of this CPU.
    ///
    /// # Safety: must be called with interrupts disabled,
    /// to prevent race with process being moved to a different CPU.
    #[inline]
    pub unsafe fn get_id() -> usize {
        tp::read()
    }

    /// Returns a mutable pointer to this CPU's [`Cpu`] struct.
    ///
    /// # Safety: must be called with interrupts disabled,
    /// to prevent race with process being moved to a different CPU.
    pub unsafe fn mycpu() -> *mut Cpu {
        assert!(!interrupts::get(), "mycpu interrupts enabled");
        let id = Self::get_id();
        CPUS.0[id].get()
    }

    /// Locks this CPU by disabling interrupts.
    /// Returns an [`InterruptLock`] as the ownership and lifetime of the lock.
    pub fn lock_mycpu() -> InterruptLock {
        let old_state = interrupts::get();
        interrupts::disable();

        unsafe { (*Self::mycpu()).lock(old_state) }
    }

    /// Returns an arc pointer to this CPU's [`Proc`].
    pub fn myproc() -> Option<Arc<Proc>> {
        let _lock = Self::lock_mycpu();

        let cpu = unsafe { &*Self::mycpu() };
        cpu.proc.as_ref().map(Arc::clone)
    }
}

pub struct InterruptLock {}

impl Drop for InterruptLock {
    fn drop(&mut self) {
        unsafe { (*Cpus::mycpu()).unlock() }
    }
}

/// Saved registers for kernel context switches.
#[repr(C)]
pub struct Context {
    pub ra: usize,
    pub sp: usize,

    // callee-saved
    pub s0: usize,
    pub s1: usize,
    pub s2: usize,
    pub s3: usize,
    pub s4: usize,
    pub s5: usize,
    pub s6: usize,
    pub s7: usize,
    pub s8: usize,
    pub s9: usize,
    pub s10: usize,
    pub s11: usize,
}

impl Context {
    pub const fn new() -> Self {
        Self {
            ra: 0,
            sp: 0,
            s0: 0,
            s1: 0,
            s2: 0,
            s3: 0,
            s4: 0,
            s5: 0,
            s6: 0,
            s7: 0,
            s8: 0,
            s9: 0,
            s10: 0,
            s11: 0,
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

pub struct PID(usize);

impl PID {
    pub fn new() -> Self {
        static PID_COUNT: AtomicUsize = AtomicUsize::new(0);
        PID(PID_COUNT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for PID {
    fn default() -> Self {
        Self::new()
    }
}

pub enum ProcState {
    Unused,
    Used,
    Sleeping,
    Runnable,
    Running,
    Zombie,
}

// Per-process state
pub struct Proc {
    lock: SpinLock,

    state: ProcState,
    chan: (),
    killed: bool,
    xstate: isize,
    pid: PID,
    // parent: *const Self,
    // TODO
}

pub fn sleep(chan: usize, lock: SpinLock) {
    let mut proc = Cpus::myproc().unwrap();
    // let mut proc_lock = proc.lock.acquire();

    // p.lock.acquire();
}
