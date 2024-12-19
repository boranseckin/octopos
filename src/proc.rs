use core::cell::UnsafeCell;
use core::mem::{MaybeUninit, transmute};
use core::sync::atomic::{AtomicUsize, Ordering};

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;

use crate::memlayout::kstack;
use crate::param::{NCPU, NPROC};
use crate::riscv::registers::tp;
use crate::riscv::{PGSIZE, PTE_R, PTE_W, interrupts};
use crate::spinlock::{Mutex, SpinLock};
use crate::vm::{KVM, PageTable, VA};

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

pub static mut PROCS: Procs = Procs::new();

pub struct Procs([MaybeUninit<Proc>; NPROC]);

impl Procs {
    const fn new() -> Self {
        Self([const { MaybeUninit::uninit() }; NPROC])
    }

    pub fn init(&mut self) {
        for (i, p) in self.0.iter_mut().enumerate() {
            let mut proc = Proc::new();
            proc.data.kstack = VA(kstack(i));
            p.write(proc);
            unsafe { p.assume_init_mut() };
        }
    }

    pub fn map_stacks(&mut self) {
        for (i, _) in self.0.iter().enumerate() {
            // TODO: This is not a page table per se but "stack" is a s big as a PGSIZE so the same
            // initializer works for now. It would be better to create a new struct called Stack...
            let pa = PageTable::new().expect("proc map stack kalloc").as_pa();
            // Cannot get va from proc.data.kstack since init function is not called yet.
            let va = VA(kstack(i));
            unsafe {
                #[allow(static_mut_refs)]
                KVM.get_mut().unwrap().map(va, pa, PGSIZE, PTE_R | PTE_W)
            };
        }
    }
}

// Per-process state
pub struct Proc {
    pub inner: Mutex<ProcInner>,
    data: ProcData,
    // TODO: parent
}

pub enum ProcState {
    Unused,
    Used,
    Sleeping,
    Runnable,
    Running,
    Zombie,
}

// Public fields for Proc, lock must be held when using these
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
            pid: PID::new(),
        }
    }
}

// Private fields for Proc
struct ProcData {
    // Virtual address of kernel stack
    kstack: VA,
    // Size of process memory (bytes)
    sz: usize,
    // User page table
    pagetable: PageTable,
    // Data page for trampoline
    trapframe: Box<TrapFrame>,
    // swtch() here to run process
    context: Context,
    // Open files
    open_files: (),
    // Current directory
    cwd: (),
    // Process name
    name: String,
}

impl ProcData {
    fn new() -> Self {
        Self {
            kstack: VA(0),
            sz: 0,
            pagetable: todo!(),
            trapframe: todo!(),
            context: Context::new(),
            open_files: (),
            cwd: (),
            name: Default::default(),
        }
    }
}

impl Proc {
    fn new() -> Self {
        Self {
            inner: Mutex::new(ProcInner::new(), "proc"),
            data: ProcData::new(),
        }
    }
}

pub fn sleep(chan: usize, lock: SpinLock) {}
