use core::cell::UnsafeCell;
use core::mem::{MaybeUninit, transmute};
use core::sync::atomic::{AtomicUsize, Ordering};

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::error::KernelError;
use crate::memlayout::{TRAMPOLINE, TRAPFRAME, kstack};
use crate::param::{NCPU, NPROC};
use crate::println;
use crate::riscv::registers::tp;
use crate::riscv::{PGSIZE, PTE_R, PTE_W, PTE_X, interrupts};
use crate::spinlock::{Mutex, MutexGuard, SpinLock};
use crate::sync::LazyLock;
use crate::trampoline::trampoline;
use crate::vm::{KVM, PageTable, Uvm, VA};

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
        InterruptLock
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
        unsafe { tp::read() }
    }

    /// Returns a mutable pointer to this CPU's [`Cpu`] struct.
    ///
    /// # Safety: must be called with interrupts disabled,
    /// to prevent race with process being moved to a different CPU.
    pub unsafe fn mycpu() -> *mut Cpu {
        unsafe {
            assert!(!interrupts::get(), "mycpu interrupts enabled");
            let id = Self::get_id();
            CPUS.0[id].get()
        }
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

pub struct InterruptLock;

impl Drop for InterruptLock {
    fn drop(&mut self) {
        unsafe { (*Cpus::mycpu()).unlock() }
    }
}

/// Saved registers for kernel context switches.
#[derive(Debug)]
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

// per-process data for the trap handling code in trampoline.S.
// sits in a page by itself just under the trampoline page in the
// user page table. not specially mapped in the kernel page table.
// uservec in trampoline.S saves user registers in the trapframe,
// then initializes registers from the trapframe's
// kernel_sp, kernel_hartid, kernel_satp, and jumps to kernel_trap.
// usertrapret() and userret in trampoline.S set up
// the trapframe's kernel_*, restore user registers from the
// trapframe, switch to the user page table, and enter user space.
// the trapframe includes callee-saved user registers like s0-s11 because the
// return-to-user path via usertrapret() doesn't return through
// the entire kernel call stack.
#[derive(Debug)]
#[repr(C, align(4096))]
pub struct TrapFrame {
    /*   0 */ pub kernel_satp: usize, // kernel page table
    /*   8 */ pub kernel_sp: usize, // top of process's kernel stack
    /*  16 */ pub kernel_trap: usize, // usertrap()
    /*  24 */ pub epc: usize, // saved user program counter
    /*  32 */ pub kernel_hartid: usize, // saved kernel tp
    /*  40 */ pub ra: usize,
    /*  48 */ pub sp: usize,
    /*  56 */ pub gp: usize,
    /*  64 */ pub tp: usize,
    /*  72 */ pub t0: usize,
    /*  80 */ pub t1: usize,
    /*  88 */ pub t2: usize,
    /*  96 */ pub s0: usize,
    /* 104 */ pub s1: usize,
    /* 112 */ pub a0: usize,
    /* 120 */ pub a1: usize,
    /* 128 */ pub a2: usize,
    /* 136 */ pub a3: usize,
    /* 144 */ pub a4: usize,
    /* 152 */ pub a5: usize,
    /* 160 */ pub a6: usize,
    /* 168 */ pub a7: usize,
    /* 176 */ pub s2: usize,
    /* 184 */ pub s3: usize,
    /* 192 */ pub s4: usize,
    /* 200 */ pub s5: usize,
    /* 208 */ pub s6: usize,
    /* 216 */ pub s7: usize,
    /* 224 */ pub s8: usize,
    /* 232 */ pub s9: usize,
    /* 240 */ pub s10: usize,
    /* 248 */ pub s11: usize,
    /* 256 */ pub t3: usize,
    /* 264 */ pub t4: usize,
    /* 272 */ pub t5: usize,
    /* 280 */ pub t6: usize,
}

impl TrapFrame {
    pub fn try_new() -> Result<Self, KernelError> {
        let memory: Box<MaybeUninit<Self>> = Box::try_new_zeroed()?;
        let memory = unsafe { memory.assume_init() };
        Ok(*memory)
    }
}

#[derive(Debug)]
pub struct PID(pub usize);

impl PID {
    pub fn alloc() -> Self {
        static PID_COUNT: AtomicUsize = AtomicUsize::new(0);
        PID(PID_COUNT.fetch_add(1, Ordering::Relaxed))
    }
}

pub static PROCS: LazyLock<Procs> = LazyLock::new(Procs::new);

pub struct Procs(pub Vec<Arc<Proc>>);

impl Procs {
    pub fn new() -> Self {
        // don't like how this turned out
        let pool = [(); NPROC]
            .iter()
            .enumerate()
            .map(|(i, _)| Arc::new(Proc::new(i)))
            .collect::<Vec<_>>();
        Self(pool)
    }

    pub unsafe fn map_stacks(&self) {
        for (i, _) in self.0.iter().enumerate() {
            // TODO: This is not a page table per se but "stack" is a s big as a PGSIZE so the same
            // initializer works for now. It would be better to create a new struct called Stack...
            let pa = PageTable::try_new().expect("proc map stack kalloc").as_pa();
            // Cannot get va from proc.data.kstack since init function is not called yet.
            let va = VA(kstack(i));
            unsafe {
                #[allow(static_mut_refs)]
                KVM.get_mut().unwrap().map(va, pa, PGSIZE, PTE_R | PTE_W)
            };
        }
    }

    /// Look in the process table for an `ProcState::Unused` proc.
    /// If found, initialize state required to run in the kernel,
    /// and return both proc and its inner mutex guard.
    pub fn alloc(&self) -> Result<(&Arc<Proc>, MutexGuard<'_, ProcInner>), KernelError> {
        for proc in &self.0 {
            let mut inner = proc.inner.lock();
            if inner.state == ProcState::Unused {
                inner.pid = PID::alloc();
                inner.state = ProcState::Used;

                let data = unsafe { proc.data_mut() };

                // Allocate a trapframe page.
                match Box::<TrapFrame>::try_new_zeroed() {
                    Ok(trapframe) => {
                        data.trapframe.replace(unsafe { trapframe.assume_init() });
                    }
                    Err(err) => {
                        proc.free(inner);
                        return Err(err.into());
                    }
                }

                // Allocate an empty user page table.
                match proc.create_pagetable() {
                    Ok(uvm) => {
                        data.pagetable = Some(uvm);
                    }
                    Err(err) => {
                        proc.free(inner);
                        return Err(err);
                    }
                }

                // Set up new context to start executing at forkret, which return to user space.
                data.context.ra = fork_ret as usize;
                data.context.sp = data.kstack.0 + PGSIZE;

                return Ok((proc, inner));
            }
        }

        // TODO: change this error to "out of free proc"
        Err(KernelError::AllocError)
    }
}

unsafe impl Sync for Procs {}

pub fn init() {
    unsafe {
        for p in PROCS.0.iter() {
            p.data_mut().kstack = VA(kstack(p.id));
        }
    }
}

// Per-process state
#[derive(Debug)]
pub struct Proc {
    id: usize,
    pub inner: Mutex<ProcInner>,
    data: UnsafeCell<ProcData>,
    // TODO: parent
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProcState {
    Unused,
    Used,
    Sleeping,
    Runnable,
    Running,
    Zombie,
}

// Public fields for Proc, lock must be held when using these
#[derive(Debug)]
pub struct ProcInner {
    // Process state
    pub state: ProcState,
    // If Some, sleeping on chan
    pub chan: Option<()>,
    // If Some, have been killed
    pub killed: bool,
    // Exit status to be returned to parent's wait
    pub xstate: i32,
    // Process ID
    pub pid: PID,
}

impl ProcInner {
    fn new() -> Self {
        Self {
            state: ProcState::Unused,
            chan: None,
            killed: false,
            xstate: 0,
            pid: PID(0),
        }
    }
}

// Private fields for Proc
#[derive(Debug)]
pub struct ProcData {
    // Virtual address of kernel stack
    pub kstack: VA,
    // Size of process memory (bytes)
    pub size: usize,
    // User page table
    pub pagetable: Option<Uvm>,
    // Data page for trampoline
    pub trapframe: Option<Box<TrapFrame>>,
    // swtch() here to run process
    pub context: Context,
    // Open files
    pub open_files: (),
    // Current directory
    pub cwd: (),
    // Process name
    pub name: String,
}

impl ProcData {
    fn new() -> Self {
        Self {
            kstack: VA(0),
            size: 0,
            pagetable: None,
            trapframe: None,
            context: Context::new(),
            open_files: (),
            cwd: (),
            name: Default::default(),
        }
    }
}

unsafe impl Sync for ProcData {}
unsafe impl Send for ProcData {}

impl Proc {
    fn new(id: usize) -> Self {
        Self {
            id,
            inner: Mutex::new(ProcInner::new(), "proc"),
            data: UnsafeCell::new(ProcData::new()),
        }
    }

    pub fn data(&self) -> &ProcData {
        unsafe { &*self.data.get() }
    }

    #[allow(clippy::mut_from_ref)]
    pub unsafe fn data_mut(&self) -> &mut ProcData {
        unsafe { &mut *self.data.get() }
    }

    /// Create a user page table using a given process's trapframe address, with no user memory,
    /// but with trampoline and trapframe pages.
    pub fn create_pagetable(&self) -> Result<Uvm, KernelError> {
        let mut uvm = Uvm::try_new()?;

        // Map the trampoline code (for system call returns) at the highest user virtual address.
        // Only the supervisor uses it, on the way to/from user space, so not PTE_U.
        if let Err(err) = uvm.map_pages(
            TRAMPOLINE.into(),
            (trampoline as usize).into(),
            PGSIZE,
            PTE_R | PTE_X,
        ) {
            uvm.free(0);
            return Err(err);
        }

        // Map the trapframe page just below the trampoline page, for `trampoline.rs`.
        let data = self.data();
        if let Err(err) = uvm.map_pages(
            TRAPFRAME.into(),
            // As disgusting as this is, we need to get the address of the trapframe.
            (data.trapframe.as_deref().unwrap() as *const _ as usize).into(),
            PGSIZE,
            PTE_R | PTE_W,
        ) {
            uvm.unmap(TRAMPOLINE.into(), 1, false);
            uvm.free(0);
            return Err(err);
        }

        Ok(uvm)
    }

    /// Free the process and the data attached to it (including user pages).
    pub fn free(&self, mut inner: MutexGuard<'_, ProcInner>) {
        let data = unsafe { self.data_mut() };

        if let Some(trapframe) = data.trapframe.take() {
            let _tf = trapframe;
        }

        if let Some(mut uvm) = data.pagetable.take() {
            uvm.free(data.size);
        }

        data.size = 0;
        inner.pid = PID(0);
        // TODO: parent
        data.name.clear();
        inner.chan = None;
        inner.killed = false;
        inner.xstate = 0;
        inner.state = ProcState::Unused;
    }
}

unsafe impl Sync for Proc {}

pub unsafe extern "C" fn fork_ret() {
    unsafe {
        static mut FIRST: bool = true;

        // Still holding process lock from scheduler.
        Cpus::myproc().unwrap().inner.force_unlock();

        if FIRST {
            FIRST = false;
            todo!("fsinit");
        }

        todo!("usertrapret")
    }
}

pub fn sleep(chan: usize, lock: SpinLock) {}
