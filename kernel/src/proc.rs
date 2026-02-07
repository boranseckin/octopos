use core::arch::asm;
use core::cell::UnsafeCell;
use core::mem::{MaybeUninit, transmute};
use core::ptr;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use alloc::boxed::Box;
use alloc::string::String;

use crate::error::KernelError;
use crate::exec::exec;
use crate::file::File;
use crate::fs::{self, Inode, Path};
use crate::log::Operation;
use crate::memlayout::{TRAMPOLINE, TRAPFRAME, kstack};
use crate::param::{NCPU, NKSTACK_PAGES, NOFILE, NPROC, ROOTDEV};
use crate::riscv::{PGSIZE, PTE_R, PTE_W, PTE_X, interrupts, registers::tp};
use crate::spinlock::{SpinLock, SpinLockGuard};
use crate::swtch::swtch;
use crate::sync::OnceLock;
use crate::trampoline::trampoline;
use crate::trap::usertrapret;
use crate::vm::{Kvm, PA, PageTable, Uvm, VA};

pub static CPU_TABLE: CpuTable = CpuTable::new();
pub static PROC_TABLE: ProcTable = ProcTable::new();
pub static INIT_PROC: OnceLock<&Proc> = OnceLock::new();

/// Per-CPU state
pub struct Cpu {
    pub proc: Option<&'static Proc>,
    pub context: Context,
    pub num_off: isize,
    pub interrupts_enabled: bool,
}

impl Cpu {
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
    pub fn unlock(&mut self) {
        assert!(!interrupts::get(), "cpu unlock - interruptible");
        assert!(self.num_off >= 1, "cpu unlock");

        self.num_off -= 1;
        if self.num_off == 0 && self.interrupts_enabled {
            interrupts::enable();
        }
    }
}

/// Table of CPUs
pub struct CpuTable([UnsafeCell<Cpu>; NCPU]);

impl CpuTable {
    /// Creates a new CPU table.
    const fn new() -> Self {
        let mut array: [MaybeUninit<_>; NCPU] = unsafe { MaybeUninit::uninit().assume_init() };
        let mut i = 0;
        while i < NCPU {
            array[i] = MaybeUninit::new(UnsafeCell::new(Cpu::new()));
            i += 1;
        }
        unsafe { transmute(array) }
    }
}

unsafe impl Sync for CpuTable {}

/// A lock that releases the CPU lock when dropped.
#[derive(Debug)]
pub struct InterruptLock;

impl Drop for InterruptLock {
    fn drop(&mut self) {
        // # Safety: we are still holding the CPU lock
        unsafe { current_cpu().unlock() }
    }
}

/// Returns the hart id of the current CPU.
///
/// # Safety
/// Must be called with interrupts disabled to prevent race with process being moved to a different CPU.
#[inline]
pub unsafe fn current_id() -> usize {
    unsafe { tp::read() }
}

/// Returns a mutable pointer to the current CPU's [`CPU`] struct.
///
/// # Safety
/// Must be called with interrupts disabled to prevent race with process being moved to a different CPU.
pub unsafe fn current_cpu() -> &'static mut Cpu {
    unsafe {
        assert!(!interrupts::get(), "mycpu interrupts enabled");
        let id = current_id();
        &mut *CPU_TABLE.0[id].get()
    }
}

/// Locks this CPU by disabling interrupts.
/// Returns an [`InterruptLock`] as the ownership and lifetime of the lock.
pub fn lock_current_cpu() -> InterruptLock {
    let old_state = interrupts::get();
    interrupts::disable();

    unsafe { current_cpu().lock(old_state) }
}

/// Returns a reference to this CPU's [`Proc`].
pub fn current_proc_opt() -> Option<&'static Proc> {
    let _lock = lock_current_cpu();

    let cpu = unsafe { current_cpu() };
    cpu.proc
}

/// Returns a reference to this CPU's [`Proc`].
/// It unwraps the option and panics if there is no current process.
pub fn current_proc() -> &'static Proc {
    current_proc_opt().expect("no current process")
}

/// Returns a shared reference to this CPU's [`Proc`] and its underlying [`ProcData`].
pub fn current_proc_and_data() -> (&'static Proc, &'static ProcData) {
    let proc = current_proc();
    let data = proc.data();
    (proc, data)
}

/// Returns a shared reference to this CPU's [`Proc`] and exclusive reference to its underlying [`ProcData`].
pub fn current_proc_and_data_mut() -> (&'static Proc, &'static mut ProcData) {
    let proc = current_proc();
    // # Safety: we are the current proc
    let data = unsafe { proc.data_mut() };
    (proc, data)
}

/// Saved registers for kernel context switches.
#[derive(Debug, Clone, Copy, Default)]
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

    pub fn zero(&mut self) {
        self.ra = 0;
        self.sp = 0;
        self.s0 = 0;
        self.s1 = 0;
        self.s2 = 0;
        self.s3 = 0;
        self.s4 = 0;
        self.s5 = 0;
        self.s6 = 0;
        self.s7 = 0;
        self.s8 = 0;
        self.s9 = 0;
        self.s10 = 0;
        self.s11 = 0;
    }
}

/// Per-process data for the trap handling code in `trampoline.rs`.
/// Sits in a page by itself just under the trampoline page in the user page table. Not specially
/// mapped in the kernel page table. `uservec` in `trampoline.rs` saves user registers in the
/// trapframe, then initializes registers from the trapframe's kernel_sp, kernel_hartid,
/// kernel_satp, and jumps to kernel_trap. `usertrapret()` and `userret()` in `trampoline.rs` set up
/// the trapframe's `kernel_*`, restore user registers from the trapframe, switch to the user page
/// table, and enter user space. the trapframe includes callee-saved user registers like s0-s11
/// because the return-to-user path via usertrapret() doesn't return through the entire kernel call
/// stack.
#[derive(Debug, Clone)]
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
        let memory: Box<MaybeUninit<Self>> = try_log!(Box::try_new_zeroed());
        let memory = unsafe { memory.assume_init() };
        Ok(*memory)
    }
}

/// Wrapper around usize to represent process IDs.
/// It must be created using `Pid::alloc()` to ensure uniqueness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pid(usize);

impl Pid {
    /// Allocates a new PID by incrementing a global counter.
    pub fn alloc() -> Self {
        static PID_COUNT: AtomicUsize = AtomicUsize::new(1);
        Pid(PID_COUNT.fetch_add(1, Ordering::Relaxed))
    }

    /// Creates a PID from a usize.
    ///
    /// # Safety
    /// The caller must ensure the `Pid` has been already allocated via `Pid::alloc()`.
    pub unsafe fn from_usize(value: usize) -> Self {
        Pid(value)
    }
}

impl core::ops::Deref for Pid {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Channel type for `sleep`/`wakeup`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// `proc.id` for `sleep()` / `wakeup()`.
    Proc(usize),
    /// System ticks
    Ticks,
    /// I/O buffer
    Buffer(usize),
    /// Lock
    Lock(usize),
    /// Log
    Log,
    /// Read end of pipe.
    PipeRead(usize),
    /// Write end of pipe.
    PipeWrite(usize),
}

/// Process control block
#[derive(Debug)]
pub struct Proc {
    /// NOT `Pid`. Used for indexing in `PROC_TABLE` and matching parent-child relationships.
    pub id: usize,
    pub inner: SpinLock<ProcInner>,
    data: UnsafeCell<ProcData>,
}

/// The state of a process.
#[derive(Debug, PartialEq, Eq, Default)]
pub enum ProcState {
    #[default]
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
#[derive(Debug, Default)]
pub struct ProcInner {
    /// Process state
    pub state: ProcState,
    /// If Some, sleeping on chan
    pub channel: Option<Channel>,
    /// If true, have been killed
    pub killed: bool,
    /// Exit status to be returned to parent's wait
    pub xstate: isize,
    /// Process ID
    pub pid: Pid,
}

impl ProcInner {
    const fn new() -> Self {
        Self {
            state: ProcState::Unused,
            channel: None,
            killed: false,
            xstate: 0,
            pid: Pid(0),
        }
    }
}

/// Private fields for Proc
#[derive(Debug, Default)]
pub struct ProcData {
    /// Virtual address of kernel stack
    pub kstack: VA,
    /// Size of process memory (bytes)
    pub size: usize,
    /// User page table
    pub pagetable: Option<Uvm>,
    /// Data page for trampoline
    pub trapframe: Option<Box<TrapFrame>>,
    /// swtch() here to run process
    pub context: Context,
    /// Open files
    pub open_files: [Option<File>; NOFILE],
    /// Current directory
    pub cwd: Inode,
    /// Process name
    pub name: String,
}

impl ProcData {
    const fn new() -> Self {
        Self {
            kstack: VA::new(0),
            size: 0,
            pagetable: None,
            trapframe: None,
            context: Context::new(),
            open_files: [const { None }; NOFILE],
            cwd: Inode::new(0, 0, 0),
            name: String::new(),
        }
    }

    /// Returns a reference to the trapframe.
    pub fn trapframe(&self) -> &TrapFrame {
        self.trapframe.as_ref().unwrap()
    }

    /// Returns a mutable reference to the trapframe.
    pub fn trapframe_mut(&mut self) -> &mut TrapFrame {
        self.trapframe.as_mut().unwrap()
    }

    /// Returns a reference to the user page table.
    pub fn pagetable(&self) -> &Uvm {
        self.pagetable.as_ref().unwrap()
    }

    /// Returns a mutable reference to the user page table.
    pub fn pagetable_mut(&mut self) -> &mut Uvm {
        self.pagetable.as_mut().unwrap()
    }

    /// Returns a mutable reference to both the pagetable and trapframe.
    pub fn pagetable_and_trapframe_mut(&mut self) -> (&mut Uvm, &mut TrapFrame) {
        (
            self.pagetable.as_mut().unwrap(),
            self.trapframe.as_mut().unwrap(),
        )
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

    /// Returns a mutable reference to the process's data.
    ///
    /// # Safety
    /// The caller must ensure they have exclusive access to the `Proc`. This is true if either
    ///     1. it's the current proc (most cases) or
    ///     2. the proc's state hasn't been set to Runnable/Sleeping yet (fork, allocproc).
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn data_mut(&self) -> &mut ProcData {
        unsafe { &mut *self.data.get() }
    }

    /// Returns true if this process is the init process.
    pub fn is_init_proc(&self) -> bool {
        ptr::eq(self, *INIT_PROC.get().unwrap())
    }

    /// Returns true if this process has been killed.
    ///
    /// Acquires and releases the proc lock.
    pub fn is_killed(&self) -> bool {
        let inner = self.inner.lock();
        inner.killed
    }

    /// Create a user page table using a given process's trapframe address, with no user memory,
    /// but with trampoline and trapframe pages.
    pub fn create_pagetable(&self) -> Result<Uvm, KernelError> {
        let mut uvm = Uvm::try_new()?;

        // Map the trampoline code (for system call returns) at the highest user virtual address.
        // Only the supervisor uses it, on the way to/from user space, so not PTE_U.
        if let Err(err) = log!(uvm.map_pages(
            TRAMPOLINE.into(),
            (trampoline as *const () as usize).into(),
            PGSIZE,
            PTE_R | PTE_X,
        )) {
            uvm.free(0);
            return Err(err.into());
        }

        // Map the trapframe page just below the trampoline page, for `trampoline.rs`.
        let data = self.data();
        if let Err(err) = log!(uvm.map_pages(
            TRAPFRAME.into(),
            PA::from(data.trapframe() as *const _ as usize),
            PGSIZE,
            PTE_R | PTE_W,
        )) {
            uvm.unmap(TRAMPOLINE.into(), 1, false);
            uvm.free(0);
            return Err(err.into());
        }

        Ok(uvm)
    }

    /// Free the process and the data attached to it (including user pages).
    pub fn free(&self, mut inner: SpinLockGuard<'_, ProcInner>) {
        // # Safety: we are the only ones with access to this proc
        let data = unsafe { self.data_mut() };

        if let Some(trapframe) = data.trapframe.take() {
            drop(trapframe);
        }

        if let Some(uvm) = data.pagetable.take() {
            uvm.proc_free(data.size);
        }

        data.size = 0;
        inner.pid = Pid(0);
        data.name.clear();
        inner.channel = None;
        inner.killed = false;
        inner.xstate = 0;
        inner.state = ProcState::Unused;
    }
}

unsafe impl Sync for Proc {}

/// Table of processes
pub struct ProcTable {
    pub table: [UnsafeCell<Proc>; NPROC],
    // instead of having a global mutex and individual parent fields on each proc, combining all
    // parents to one array guarded by a mutex is better.
    // parents[child.id] == Some(parent.id)
    pub parents: SpinLock<[Option<usize>; NPROC]>,
}

impl ProcTable {
    pub const fn new() -> Self {
        let mut table: [MaybeUninit<UnsafeCell<Proc>>; NPROC] =
            unsafe { MaybeUninit::uninit().assume_init() };

        let mut i = 0;
        while i < NPROC {
            table[i] = MaybeUninit::new(UnsafeCell::new(Proc::new(i)));
            i += 1;
        }

        Self {
            table: unsafe {
                transmute::<[MaybeUninit<UnsafeCell<Proc>>; 64], [UnsafeCell<Proc>; 64]>(table)
            },
            parents: SpinLock::new([None; NPROC], "parents"),
        }
    }

    /// Returns a reference to the process at the given index.
    pub fn get(&self, index: usize) -> &Proc {
        unsafe { &*self.table[index].get() }
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
    ///
    /// # Safety
    /// The caller must ensure that the kernel page table is not used concurrently.
    /// Which should be the case when initializing the page.
    pub unsafe fn map_stacks(&self, kvm: &mut Kvm) {
        for (i, _) in self.table.iter().enumerate() {
            let base_va = VA::from(kstack(i));

            for page in 0..NKSTACK_PAGES {
                // TODO: This is not a page table per se but "stack" is a s big as a PGSIZE so the
                // same initializer works for now.
                let pa = log!(PageTable::try_new())
                    .expect("proc map stack kalloc")
                    .as_pa();
                let va = base_va + page * PGSIZE;
                kvm.map(va, pa, PGSIZE, PTE_R | PTE_W);
            }
        }
    }

    /// Searches the process table for an `ProcState::Unused` proc.
    /// If found, initialize state required to run in the kernel, and return both proc and its
    /// inner mutex guard.
    pub fn alloc(&self) -> Result<(&Proc, SpinLockGuard<'_, ProcInner>), KernelError> {
        for proc in self.iter() {
            let mut inner = proc.inner.lock();
            if inner.state == ProcState::Unused {
                inner.pid = Pid::alloc();
                inner.state = ProcState::Used;

                // # Safety: proc is not yet runnable, so we are the only ones with access to it
                let data = unsafe { proc.data_mut() };

                // Allocate a trapframe page.
                match log!(Box::<TrapFrame>::try_new_zeroed()) {
                    Ok(trapframe) => {
                        data.trapframe.replace(unsafe { trapframe.assume_init() });
                    }
                    Err(err) => {
                        proc.free(inner);
                        return Err(err.into());
                    }
                }

                // Allocate an empty user page table.
                match log!(proc.create_pagetable()) {
                    Ok(uvm) => {
                        data.pagetable.replace(uvm);
                    }
                    Err(err) => {
                        proc.free(inner);
                        return Err(err);
                    }
                }

                // Set up new context to start executing at forkret, which returns to user space.
                data.context.zero();
                data.context.ra = fork_ret as *const () as usize;
                data.context.sp = (data.kstack + NKSTACK_PAGES * PGSIZE).as_usize();

                return Ok((proc, inner));
            }
        }

        Err(KernelError::OutOfProc)
    }

    /// Prints a process listing to the console.
    /// For debugging only, it does not lock to avoid creating more problems.
    pub unsafe fn dump(&self) {
        println!("");

        for proc in &self.table {
            let proc = unsafe { &*proc.get() };
            let inner = unsafe { proc.inner.get_mut_unchecked() };
            if inner.state == ProcState::Unused {
                continue;
            }

            println!("{} {:?} {}", inner.pid.0, inner.state, proc.data().name);
        }
    }
}

unsafe impl Sync for ProcTable {}

/// Sets up first user process.
pub fn user_init() {
    let (proc, mut inner) = log!(PROC_TABLE.alloc()).unwrap();
    INIT_PROC.initialize(|| Ok::<_, ()>(proc));

    // # Safety: during initialization, we are the only ones with access to this proc
    let data = unsafe { proc.data_mut() };

    data.cwd = log!(Path::new("/").resolve()).expect("root path to exist");

    inner.state = ProcState::Runnable;

    // inner lock is dropped
}

/// Grows or shrinks user memory by `n` bytes.
/// The new size is reflected in `proc.data.size` and return.
///
/// # Safety
/// The caller must ensure exclusive access to the process's memory.
pub unsafe fn grow(n: isize) -> Result<usize, KernelError> {
    let (_proc, data) = current_proc_and_data_mut();

    let mut size = data.size;

    if n > 0 {
        size = try_log!(data.pagetable_mut().alloc(size, size + (n as usize), PTE_W));
    } else if n < 0 {
        let shrink = (-n) as usize;
        if shrink > size {
            err!(KernelError::InvalidArgument);
        }

        size = data.pagetable_mut().dealloc(size, size - shrink);
    }

    data.size = size;
    Ok(size)
}

/// Crates a new process, copying the parent.
/// Sets up the child kernel stack to return as if from `fork()` system call.
pub fn fork() -> Result<Pid, KernelError> {
    let (proc, data) = current_proc_and_data_mut();

    // allocate process
    let (new_proc, new_inner) = try_log!(PROC_TABLE.alloc());
    // # Safety: new_proc is not yet runnable, so we are the only ones with access to it
    let new_data = unsafe { new_proc.data_mut() };

    // copy user memory from parent to child
    let new_pagetable = new_data.pagetable_mut();
    let size = data.size;
    if let Err(err) = log!(data.pagetable_mut().copy(new_pagetable, size)) {
        new_proc.free(new_inner);
        return Err(err.into());
    };
    new_data.size = data.size;

    // copy saved user registers
    let new_trapframe = new_data.trapframe_mut();
    let trapframe = data.trapframe();
    new_trapframe.clone_from(trapframe);

    // cause fork to return 0 in the child
    new_trapframe.a0 = 0;

    // increment reference counts on open file descriptors
    for (i, file) in data.open_files.iter_mut().enumerate() {
        if let Some(file) = file.as_mut() {
            new_data.open_files[i] = Some(file.dup());
        }
    }
    new_data.cwd = data.cwd.dup();

    new_data.name = data.name.clone();

    let pid = new_inner.pid;

    // drop new proc's lock here
    drop(new_inner);

    {
        let mut parents = PROC_TABLE.parents.lock();
        parents[new_proc.id] = Some(proc.id);
    }

    // re-acquire new proc's lock
    let mut new_inner = new_proc.inner.lock();
    new_inner.state = ProcState::Runnable;

    Ok(pid)
}

/// Passes `original`'s abandoned children to init.
pub fn reparent(original: &Proc, parents: &mut SpinLockGuard<'_, [Option<usize>; NPROC]>) {
    for proc in parents.iter_mut() {
        if *proc == Some(original.id) {
            *proc = Some(INIT_PROC.get().unwrap().id);
            wakeup(Channel::Proc(INIT_PROC.get().unwrap().id));
        }
    }
}

/// Exits the current process and does not return.
///
/// An exited process remains in the zombie state until its parent calls `wait`.
pub fn exit(status: isize) -> ! {
    let (proc, data) = current_proc_and_data_mut();
    assert!(!proc.is_init_proc(), "init exiting");

    // close all open files
    for file in &mut data.open_files {
        if let Some(mut file) = file.take() {
            file.close();
        }
    }

    {
        let _op = Operation::begin();
        let cwd = data.cwd.clone();
        cwd.put();
    }

    let mut parents = PROC_TABLE.parents.lock();

    // give any children to init
    reparent(proc, &mut parents);

    // parent might be sleeping in `wait`
    let parent_id = parents[proc.id].expect("exit no parent");
    wakeup(Channel::Proc(parent_id));

    let mut inner = proc.inner.lock();
    inner.xstate = status;
    inner.state = ProcState::Zombie;

    // unlock parents
    drop(parents);

    sched(inner, &mut data.context);

    unreachable!("zombie exit");
}

/// Waits for a child process to exit and return its pid or None if there are no children.
pub fn wait(addr: VA) -> Option<Pid> {
    let current_proc = current_proc();
    let current_id = current_proc.id;

    // analogous to wait_lock
    let mut parents = PROC_TABLE.parents.lock();

    loop {
        let mut have_kids = false;

        // Scan through table looking for exited children.
        for proc in PROC_TABLE.iter() {
            if parents[proc.id] == Some(current_id) {
                // make sure the child isn't still in exit() or swtch().
                let inner = proc.inner.lock();

                have_kids = true;

                if inner.state == ProcState::Zombie {
                    let pid = inner.pid.0;

                    if addr != 0 {
                        let xstate_bytes = &inner.xstate.to_le_bytes();
                        log!(
                            // # Safety: we are the current proc
                            unsafe { current_proc.data_mut() }
                                .pagetable_mut()
                                .copy_to(xstate_bytes, addr)
                        )
                        .expect("wait copy out xstate");
                    }

                    // clear the parent relationship
                    parents[proc.id] = None;

                    proc.free(inner);

                    return Some(Pid(pid));
                }
            }
        }

        // No point waiting if we don't have any children.
        if !have_kids || current_proc.inner.lock().killed {
            return None;
        }

        // Wait for a child to exit.
        parents = sleep(Channel::Proc(current_id), parents);
    }
}

/// Per-CPU process scheduler.
/// Each CPU calls `scheduler` after setting itself up.
/// Scheduler never returns. It loops, doing:
///     - choose a process to run.
///     - swtch to start running that process.
///     - eventually that process transfers control via swtch back to the scheduler.
///
/// # Safety
/// Must be called with interrupts disabled.
pub unsafe fn scheduler() -> ! {
    // cpu does not change throughout the lifetime of the scheduler
    let cpu = unsafe { current_cpu() };

    cpu.proc.take();

    loop {
        // The most recent process to run may have had interrupts turned off; enable them to avoid
        // a deadlock if all processes are waiting. Then, turn them off to avoid possible rece
        // between an interrupt and wfi.
        interrupts::enable();
        interrupts::disable();

        let mut found = false;

        for proc in PROC_TABLE.iter() {
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
    let cpu = unsafe { current_cpu() };

    // make sure that interrupts are disabled and there are no nested locks.
    assert_eq!(cpu.num_off, 1, "sched locks");
    // make sure the process is not running before switch.
    assert_ne!(proc_inner.state, ProcState::Running, "sched running");

    // make sure that interrupts are disabled in the hardware.
    // this is to verify the software check done with num_off.
    assert!(!interrupts::get(), "sched interruptable");

    let interrupts_enabled = cpu.interrupts_enabled;
    unsafe { swtch(context, &cpu.context) };

    // get current cpu again since the process may have been moved to a different cpu.
    let cpu = unsafe { current_cpu() };
    cpu.interrupts_enabled = interrupts_enabled;

    proc_inner
}

/// Gives up the CPU for one scheduling round.
pub fn r#yield() {
    let (proc, data) = current_proc_and_data_mut();

    // proc lock will be held until after the call to the sched.
    let mut inner = proc.inner.lock();
    inner.state = ProcState::Runnable;

    sched(inner, &mut data.context);
}

/// Entry point for forked child process.
///
/// # Safety
/// This function is not called directly, but used as the return address for context switch.
pub unsafe extern "C" fn fork_ret() {
    // This is atomic since multiple CPUs could schedule their first process simultaneously.
    static FIRST: AtomicBool = AtomicBool::new(true);

    // Still holding process lock from scheduler.
    unsafe { current_proc().inner.force_unlock() };

    if FIRST
        .compare_exchange(true, false, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        // file system initialization must be run in the context of a regular process (because it
        // calls sleep), and thus cannot be run from `main()`.
        fs::init(ROOTDEV);

        println!("\nexec init\n");

        // we can invoke `exec()` now that file system is initialized.
        match log!(exec(&Path::new("/init"), &["init"])) {
            Ok(result) => {
                // # Safety: we are the current proc
                let trapframe = unsafe { current_proc().data_mut() }.trapframe_mut();
                trapframe.a0 = result;
            }
            Err(_) => panic!("fork_ret exec"),
        }
    }

    // return to user space, mimicking `usertrap()`'s return
    unsafe { usertrapret() };
}

/// Atomically releases a condition's lock and sleeps on channel.
/// Reacquires the condition's lock when awakened.
pub fn sleep<T>(channel: Channel, condition_lock: SpinLockGuard<'_, T>) -> SpinLockGuard<'_, T> {
    // To make sure the condition is not resolved before we sleep, we acquire proc's lock before
    // unlocking the condition's lock. `wakeup()` must also acquire proc's lock to resolve the
    // condition, which it cannot do before we release it.
    let condition_mutex;
    {
        let proc = current_proc();
        let mut inner = proc.inner.lock();

        condition_mutex = SpinLock::unlock(condition_lock);

        // go to sleep.
        inner.channel = Some(channel);
        inner.state = ProcState::Sleeping;

        // this is where we switch to scheduler (to another proc).
        // # Safety: we are the current proc
        let context = unsafe { &mut proc.data_mut().context };
        inner = sched(inner, context);
        // this is where we switch back to the original proc.

        inner.channel = None;
    } // drop inner lock

    // reacquire original lock.
    condition_mutex.lock()
}

/// Wakes up all processes sleeping on channel.
/// Must be called without any proc lock.
pub fn wakeup(channel: Channel) {
    // do not unwrap current proc here, since it might not exist if it is called from the
    // scheduler's context.
    let current_proc = current_proc_opt();

    for proc in PROC_TABLE.iter() {
        if current_proc.is_some_and(|p| ptr::eq(p, proc)) {
            continue;
        }

        let mut inner = proc.inner.lock();
        if inner.state == ProcState::Sleeping && inner.channel == Some(channel) {
            inner.state = ProcState::Runnable;
        }
    }
}

/// Kills the process with the given pid.
///
/// The victim won't exit until it tries to return to user space (see `usertrap()` in trap.rs).
pub fn kill(pid: Pid) -> bool {
    for proc in PROC_TABLE.iter() {
        let mut inner = proc.inner.lock();
        if inner.pid == pid {
            inner.killed = true;

            if inner.state == ProcState::Sleeping {
                // wakeup process from `sleep()`
                inner.state = ProcState::Runnable;
            }

            return true;
        }
    }

    false
}

/// Copies from kernel to user space.
pub fn copy_to_user(src: &[u8], dst: VA) -> Result<(), KernelError> {
    log!(
        // # Safety: we are the current proc
        unsafe { current_proc().data_mut() }
            .pagetable_mut()
            .copy_to(src, dst)
    )
    .map_err(|e| e.into())
}

/// Copies from user to kernel space.
pub fn copy_from_user(src: VA, dst: &mut [u8]) -> Result<(), KernelError> {
    log!(
        // # Safety: we are the current proc
        unsafe { current_proc().data_mut() }
            .pagetable_mut()
            .copy_from(src, dst)
    )
    .map_err(|e| e.into())
}

/// Initializes the process table.
///
/// # Safety
/// Must be called only once during kernel initialization.
pub unsafe fn init() {
    for proc in PROC_TABLE.iter() {
        // # Safety: we are during initialization, so we are the only ones with access to the proc
        unsafe { proc.data_mut() }.kstack = VA::from(kstack(proc.id));
    }

    println!("proc init");
}
