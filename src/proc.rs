use core::arch::asm;
use core::cell::UnsafeCell;
use core::mem::{MaybeUninit, transmute};
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use alloc::boxed::Box;
use alloc::string::String;

use crate::error::KernelError;
use crate::memlayout::{TRAMPOLINE, TRAPFRAME, kstack};
use crate::param::{NCPU, NPROC};
use crate::println;
use crate::riscv::registers::tp;
use crate::riscv::{PGSIZE, PTE_R, PTE_W, PTE_X, interrupts};
use crate::spinlock::{SpinLock, SpinLockGuard};
use crate::swtch::swtch;
use crate::sync::OnceLock;
use crate::trampoline::trampoline;
use crate::trap::usertrapret;
use crate::vm::{Kvm, PageTable, Uvm, VA};

pub static CPU_POOL: CPUPool = CPUPool::new();

/// Per-CPU state
pub struct CPU {
    pub proc: Option<&'static Proc>,
    pub context: Context,
    pub num_off: isize,
    pub interrupts_enabled: bool,
}

impl CPU {
    const fn new() -> Self {
        Self {
            proc: None,
            context: Context::new(),
            num_off: 0,
            interrupts_enabled: false,
        }
    }

    /// Locks this CPU by disabling interrupts.
    fn lock(&mut self, old_state: bool) -> InterruptLock {
        if self.num_off == 0 {
            self.interrupts_enabled = old_state;
        }
        self.num_off += 1;
        InterruptLock
    }

    /// Unlocks this CPU by enabling interrupts if appropriate.
    fn unlock(&mut self) {
        assert!(!interrupts::get(), "cpu unlock - interruptible");
        assert!(self.num_off >= 1, "cpu unlock");

        self.num_off -= 1;
        if self.num_off == 0 && self.interrupts_enabled {
            interrupts::enable();
        }
    }
}

/// Pool of CPUs.
pub struct CPUPool([UnsafeCell<CPU>; NCPU]);

impl CPUPool {
    /// Creates a new CPU pool.
    const fn new() -> Self {
        let mut array: [MaybeUninit<_>; NCPU] = unsafe { MaybeUninit::uninit().assume_init() };
        let mut i = 0;
        while i < NCPU {
            array[i] = MaybeUninit::new(UnsafeCell::new(CPU::new()));
            i += 1;
        }
        unsafe { transmute(array) }
    }

    /// Returns the hart id of the current CPU.
    ///
    /// # Safety: must be called with interrupts disabled to prevent race with process being moved to a different CPU.
    #[inline]
    pub unsafe fn current_id(&self) -> usize {
        unsafe { tp::read() }
    }

    /// Returns a mutable pointer to the current CPU's [`CPU`] struct.
    ///
    /// # Safety: must be called with interrupts disabled to prevent race with process being moved to a different CPU.
    pub unsafe fn current(&self) -> &'static mut CPU {
        unsafe {
            assert!(!interrupts::get(), "mycpu interrupts enabled");
            let id = self.current_id();
            &mut *CPU_POOL.0[id].get()
        }
    }

    /// Locks this CPU by disabling interrupts.
    /// Returns an [`InterruptLock`] as the ownership and lifetime of the lock.
    pub fn lock_current(&self) -> InterruptLock {
        let old_state = interrupts::get();
        interrupts::disable();

        unsafe { self.current().lock(old_state) }
    }

    /// Returns a reference to this CPU's [`Proc`].
    pub fn current_proc(&self) -> Option<&'static Proc> {
        let _lock = self.lock_current();

        let cpu = unsafe { &*self.current() };
        cpu.proc
    }
}

unsafe impl Sync for CPUPool {}

/// A lock that releases the CPU lock when dropped.
#[derive(Debug)]
pub struct InterruptLock;

impl Drop for InterruptLock {
    fn drop(&mut self) {
        unsafe { (*CPU_POOL.current()).unlock() }
    }
}

/// Saved registers for kernel context switches.
#[derive(Debug, Clone, Copy)]
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

/// Wrapper around usize to represent process IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PID(usize);

impl PID {
    pub fn alloc() -> Self {
        static PID_COUNT: AtomicUsize = AtomicUsize::new(0);
        PID(PID_COUNT.fetch_add(1, Ordering::Relaxed))
    }
}

impl core::ops::Deref for PID {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl core::ops::DerefMut for PID {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Process control block
#[derive(Debug)]
pub struct Proc {
    pub id: usize,
    pub inner: SpinLock<ProcInner>,
    data: UnsafeCell<ProcData>,
}

/// The state of a process.
#[derive(Debug, PartialEq, Eq)]
pub enum ProcState {
    Unused,
    Used,
    Sleeping,
    Runnable,
    Running,
    Zombie,
}

/// Public fields for Proc
///
/// Process lock must be held when accessing these.
#[derive(Debug)]
pub struct ProcInner {
    // Process state
    pub state: ProcState,
    // If Some, sleeping on chan (any const pointer to a struct)
    pub chan: usize,
    // If Some, have been killed
    pub killed: bool,
    // Exit status to be returned to parent's wait
    pub xstate: i32,
    // Process ID
    pub pid: PID,
}

impl ProcInner {
    const fn new() -> Self {
        Self {
            state: ProcState::Unused,
            chan: 0,
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
    const fn new() -> Self {
        Self {
            kstack: VA(0),
            size: 0,
            pagetable: None,
            trapframe: None,
            context: Context::new(),
            open_files: (),
            cwd: (),
            name: String::new(),
        }
    }
}

unsafe impl Sync for ProcData {}
unsafe impl Send for ProcData {}

impl Proc {
    const fn new(id: usize) -> Self {
        Self {
            id,
            inner: SpinLock::new(ProcInner::new(), "proc"),
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

    pub fn is_init_proc(&self) -> bool {
        ptr::eq(self, *INIT_PROC.get().unwrap())
    }

    /// Create a user page table using a given process's trapframe address, with no user memory,
    /// but with trampoline and trapframe pages.
    pub fn create_pagetable(&self) -> Result<Uvm, KernelError> {
        let mut uvm = Uvm::try_new()?;

        // Map the trampoline code (for system call returns) at the highest user virtual address.
        // Only the supervisor uses it, on the way to/from user space, so not PTE_U.
        if let Err(err) = uvm.map_pages(
            TRAMPOLINE.into(),
            (trampoline as *const () as usize).into(),
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
    pub fn free(&self, mut inner: SpinLockGuard<'_, ProcInner>) {
        let data = unsafe { self.data_mut() };

        if let Some(trapframe) = data.trapframe.take() {
            let _tf = trapframe;
        }

        if let Some(mut uvm) = data.pagetable.take() {
            uvm.free(data.size);
        }

        data.size = 0;
        inner.pid = PID(0);
        data.name.clear();
        inner.chan = 0;
        inner.killed = false;
        inner.xstate = 0;
        inner.state = ProcState::Unused;
    }
}

unsafe impl Sync for Proc {}

pub static PROC_POOL: ProcPool = ProcPool::new();
pub static INIT_PROC: OnceLock<&Proc> = OnceLock::new();

/// Pool of processes.
pub struct ProcPool {
    pub pool: [UnsafeCell<Proc>; NPROC],
    // instead of having a global mutex and individual parent fields on each proc, combining all
    // parents to one vector guarded by a mutex is better.
    // parents[child.id] == Some(parent.id)
    pub parents: SpinLock<[Option<usize>; NPROC]>,
}

impl ProcPool {
    pub const fn new() -> Self {
        let mut pool: [MaybeUninit<UnsafeCell<Proc>>; NPROC] =
            unsafe { MaybeUninit::uninit().assume_init() };

        let mut i = 0;
        while i < NPROC {
            pool[i] = MaybeUninit::new(UnsafeCell::new(Proc::new(i)));
            i += 1;
        }

        Self {
            pool: unsafe {
                transmute::<[MaybeUninit<UnsafeCell<Proc>>; 64], [UnsafeCell<Proc>; 64]>(pool)
            },
            parents: SpinLock::new([None; NPROC], "parents"),
        }
    }

    /// Returns a reference to the process at the given index.
    pub fn get(&self, index: usize) -> &Proc {
        unsafe { &*self.pool[index].get() }
    }

    /// Returns an iterator over all processes.
    pub fn iter(&self) -> impl Iterator<Item = &Proc> {
        (0..NPROC).map(|i| self.get(i))
    }

    /// Allocates a page for each process's kernel stack and maps it into the kernel page table.
    ///
    /// The page is mapped high in memory and followed by an invalid guard page.
    ///
    /// This is only called during KVM initialization, so the mutable reference is passed by the
    /// callee (`Kvm::make`).
    pub unsafe fn map_stacks(&self, kvm: &mut Kvm) {
        for (i, _) in self.pool.iter().enumerate() {
            // TODO: This is not a page table per se but "stack" is a s big as a PGSIZE so the same
            // initializer works for now. It would be better to create a new struct called Stack...
            let pa = PageTable::try_new().expect("proc map stack kalloc").as_pa();
            // Cannot get va from proc.data.kstack since init function is not called yet.
            let va = VA(kstack(i));

            kvm.map(va, pa, PGSIZE, PTE_R | PTE_W);
        }
    }

    /// Searches the process table for an `ProcState::Unused` proc.
    /// If found, initialize state required to run in the kernel, and return both proc and its
    /// inner mutex guard.
    pub fn alloc(&self) -> Result<(&Proc, SpinLockGuard<'_, ProcInner>), KernelError> {
        for proc in self.iter() {
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
                data.context.ra = fork_ret as *const () as usize;
                data.context.sp = data.kstack.0 + PGSIZE;

                return Ok((proc, inner));
            }
        }

        // TODO: change this error to "out of free proc"
        Err(KernelError::Alloc)
    }
}

unsafe impl Sync for ProcPool {}

/// Sets up first user process.
pub fn user_init() {
    let (proc, inner) = PROC_POOL.alloc().unwrap();
    INIT_PROC.initialize(|| Ok::<_, ()>(proc));

    // allocate one user page and copy initcode's instructions and data into it.
    todo!()
}

/// Exits the current process and does not return.
///
/// An exited process remains in the zombie state until its parent calls `wait`.
pub fn exit(status: i32) -> ! {
    let proc = CPU_POOL.current_proc().unwrap();
    assert!(!proc.is_init_proc(), "init exiting");

    let data = unsafe { proc.data_mut() };

    // Close all open files.
    todo!();

    // TODO: wait_lock

    // Give any children to init.
    todo!("reparent");

    // Parent might be sleeping in `wait`.
    wakeup(PROC_POOL.parents.lock()[proc.id].expect("exit no parent"));

    let mut inner = proc.inner.lock();
    inner.xstate = status;
    inner.state = ProcState::Zombie;

    sched(inner, &mut data.context);

    unreachable!("zombie exit");
}

/// Waits for a child process to exit and return its pid or None if there are no children.
pub fn wait(addr: VA) -> Option<usize> {
    let current_proc = CPU_POOL.current_proc().unwrap();
    let current_id = current_proc.id;

    // analogous to wait_lock
    let mut parents = PROC_POOL.parents.lock();

    loop {
        let mut have_kids = false;

        // Scan through table looking for exited children.
        for proc in PROC_POOL.iter() {
            if parents[proc.id] == Some(current_id) {
                // make sure the child isn't still in exit() or swtch().
                let inner = proc.inner.lock();

                have_kids = true;

                if inner.state == ProcState::Zombie {
                    let pid = inner.pid.0;

                    if (addr.0 != 0) {
                        unsafe {
                            let xstate_bytes = &inner.xstate.to_le_bytes();
                            current_proc
                                .data_mut()
                                .pagetable
                                .as_mut()
                                .unwrap()
                                .copy_out(addr, xstate_bytes)
                                .expect("wait copy out xstate");
                        }
                    }

                    // clear the parent relationship
                    parents[proc.id] = None;

                    proc.free(inner);

                    return Some(pid);
                }
            }
        }

        // No point waiting if we don't have any children.
        if !have_kids || current_proc.inner.lock().killed {
            return None;
        }

        // Wait for a child to exit.
        parents = sleep(current_proc as *const _ as usize, parents);
    }
}

/// Per-CPU process scheduler.
/// Each CPU calls `scheduler` after setting itself up.
/// Scheduler never returns.  It loops, doing:
///  - choose a process to run.
///  - swtch to start running that process.
///  - eventually that process transfers control via swtch back to the scheduler.
pub fn scheduler() -> ! {
    let cpu = unsafe { &mut *CPU_POOL.current() };

    cpu.proc.take();

    loop {
        // The most recent process to run may have had interrupts turned off; enable them to avoid
        // a deadlock if all processes are waiting.
        interrupts::enable();

        let mut found = false;

        for proc in PROC_POOL.iter() {
            let mut inner = proc.inner.lock();

            if inner.state == ProcState::Runnable {
                // Switch to chosen process. It is the process's job to release its lock and then
                // reacquire it before jumping back to us.
                inner.state = ProcState::Running;
                cpu.proc.replace(proc);
                unsafe { swtch(&mut cpu.context, &proc.data().context) };

                // Process is done running for now.
                // It should have changed its p->state before coming back.
                cpu.proc.take();
                found = true;
            }
        }

        if !found {
            // nothing to run; stop running on this core until an interrupt.
            interrupts::enable();

            unsafe { asm!("wfi") };
        }
    }
}

/// Switch to scheduler.
///
/// Must hold only `proc.inner` lock and have changed `proc.inner.state`.
///
/// Saves and restores `interrupts_enabled` because `interrupts_enabled` is a property of this
/// kernel thread, not this CPU.
/// It should be proc->intena and proc->noff, but that would break in the few places where a lock is
/// held but there's no process.
pub fn sched<'a>(
    proc_inner: SpinLockGuard<'a, ProcInner>,
    context: &mut Context,
) -> SpinLockGuard<'a, ProcInner> {
    let cpu = unsafe { &mut *CPU_POOL.current() };

    // might not be needed since we are passing the guard
    // assert!(!guard.holding(), "sched proc lock");

    // make sure that interrupts are disabled and there are no nested locks.
    assert_eq!(cpu.num_off, 1, "sched locks");
    // make sure the process is not running before switch.
    assert_ne!(proc_inner.state, ProcState::Running, "sched running");

    // make sure that interrupts are disabled in the hardware.
    // this is to verify the software check done with num_off.
    assert!(!interrupts::get(), "sched interruptable");

    let interrupts_enabled = cpu.interrupts_enabled;
    unsafe { swtch(context, &cpu.context) };

    cpu.interrupts_enabled = interrupts_enabled;

    proc_inner
}

/// Gives up the CPU for one scheduling round.
pub fn r#yield() {
    let proc = CPU_POOL.current_proc().unwrap();

    // proc lock will be held until after the call to the sched.
    let mut inner = proc.inner.lock();
    inner.state = ProcState::Runnable;

    let context = unsafe { &mut proc.data_mut().context };
    sched(inner, context);
}

/// Entry point for forked child process.
pub unsafe extern "C" fn fork_ret() {
    static FIRST: AtomicBool = AtomicBool::new(true);

    unsafe {
        // Still holding process lock from scheduler.
        CPU_POOL.current_proc().unwrap().inner.force_unlock();
    }

    // TODO: not sure if atomic is needed
    if FIRST
        .compare_exchange(true, false, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        todo!("fsinit");
    }

    unsafe {
        usertrapret();
    }
}

/// Atomically releases a condition's lock and sleeps on chan.
/// Reacquires the condition's lock when awakened.
pub fn sleep<T>(chan: usize, mut condition_lock: SpinLockGuard<'_, T>) -> SpinLockGuard<'_, T> {
    // To make sure the condition is not resolved before we sleep, we acquire proc's lock before
    // unlocking the condition's lock. `wakeup` function must also acquire proc's lock to resolve
    // the condition, which it cannot do before we release it.

    let condition_mutex;
    {
        let proc = CPU_POOL.current_proc().unwrap();
        let mut inner = proc.inner.lock();

        condition_mutex = SpinLock::unlock(condition_lock);

        // go to sleep.
        inner.chan = chan;
        inner.state = ProcState::Sleeping;

        // this is where we switch to scheduler (to another proc).
        let context = unsafe { &mut proc.data_mut().context };
        inner = sched(inner, context);
        // this is where we switch back to the original proc.

        inner.chan = 0;
    } // drop inner lock

    // reacquire original lock.
    condition_mutex.lock()
}

/// Wakes up all processes sleeping on chan. Must be called without any proc lock.
pub fn wakeup(chan: usize) {
    let current_proc = CPU_POOL.current_proc();

    for proc in PROC_POOL.iter() {
        if current_proc.is_some_and(|p| ptr::eq(p, proc)) {
            continue;
        }

        let mut inner = proc.inner.lock();
        if inner.state == ProcState::Sleeping && inner.chan == chan {
            inner.state = ProcState::Runnable;
        }
    }
}

/// Kills the process with the given pid.
///
/// The victim won't exit until it tries to return to user space (see `usertrap` in trap.rs).
pub fn kill(pid: PID) -> bool {
    for proc in PROC_POOL.iter() {
        let mut inner = proc.inner.lock();
        if inner.pid == pid {
            inner.killed = true;

            if inner.state == ProcState::Sleeping {
                // Wake process from `sleep()`.
                inner.state = ProcState::Runnable;
            }

            return true;
        }
    }

    false
}

pub fn init() {
    unsafe {
        for proc in PROC_POOL.iter() {
            proc.data_mut().kstack = VA(kstack(proc.id));
        }
    }

    println!("proc init");
}
