#![allow(static_mut_refs)]

use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::iter::Once;
use core::mem::MaybeUninit;
use core::ops::{Add, Deref, DerefMut, Index, IndexMut, Sub};

use crate::memlayout::{KERNBASE, PHYSTOP, PLIC, TRAMPOLINE, UART0, VIRTIO0};
use crate::proc::PROCS;
use crate::riscv::{
    self, pa_to_pte, pg_round_down, pte_to_pa, px,
    registers::{satp, vma},
    MAXVA, PGSIZE, PTE_R, PTE_V, PTE_W, PTE_X,
};
use crate::sync::OnceLock;
use crate::trampoline::trampoline;

// kernel.ld sets this to end of kernel code
extern "C" {
    fn etext();
}

pub static mut KVM: OnceLock<Kvm> = OnceLock::new();

#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub struct PA(pub usize);

#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub struct VA(pub usize);

#[repr(C, align(4096))]
#[derive(Debug, Clone)]
struct Page([u8; 4096]);

#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
struct PageTableEntry(usize);

impl PageTableEntry {
    fn is_v(&self) -> bool {
        self.0 & PTE_V == 1
    }

    fn from_pa(pa: PA) -> Self {
        Self(pa_to_pte(pa.0))
    }

    fn as_pa(&self) -> PA {
        PA(pte_to_pa(self.0))
    }
}

#[repr(C, align(4096))]
#[derive(Debug, Clone)]
struct RawPageTable([PageTableEntry; 512]);

impl RawPageTable {
    fn try_new() -> Result<*mut Self, core::alloc::AllocError> {
        let memory: Box<MaybeUninit<RawPageTable>> = Box::try_new_zeroed()?;
        let memory = unsafe { memory.assume_init() };
        Ok(Box::into_raw(memory))
    }
}

impl Deref for RawPageTable {
    type Target = [PageTableEntry; 512];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RawPageTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Index<usize> for RawPageTable {
    type Output = PageTableEntry;
    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<usize> for RawPageTable {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

#[derive(Debug, Clone)]
pub struct PageTable {
    ptr: *mut RawPageTable,
}

impl PageTable {
    pub fn new() -> Result<Self, core::alloc::AllocError> {
        Ok(Self {
            ptr: RawPageTable::try_new()?,
        })
    }

    fn from_pa(pa: PA) -> Self {
        Self {
            ptr: pa.0 as *mut RawPageTable,
        }
    }

    pub fn as_pa(&self) -> PA {
        PA(self.ptr as usize)
    }

    fn walk(&mut self, va: VA, alloc: bool) -> Option<&mut PageTableEntry> {
        assert!(va.0 < MAXVA, "walk");

        let mut pagetable = self.ptr;

        unsafe {
            for level in (1..=2).rev() {
                let pte = (*pagetable).get_mut(px(level, va.0))?;

                if pte.is_v() {
                    pagetable = pte.as_pa().0 as *mut RawPageTable;
                } else {
                    if !alloc {
                        return None;
                    }

                    pagetable = RawPageTable::try_new().expect("walk page allocation");
                    pte.0 = pa_to_pte(pagetable as usize) | PTE_V;
                }
            }

            Some((*pagetable).get_mut(px(0, va.0)).unwrap())
        }
    }

    fn map_pages(&mut self, va: VA, pa: PA, size: usize, perm: usize) -> Result<(), ()> {
        assert_ne!(size, 0, "map_pages: size");

        let last = pg_round_down(va.0 + size - 1);
        let mut va = VA(pg_round_down(va.0));
        let mut pa = pa.0;

        loop {
            if let Some(pte) = self.walk(va, true) {
                assert!(!pte.is_v(), "map_pages: remap");

                pte.0 = pa_to_pte(pa) | perm | PTE_V;

                if va.0 == last {
                    break;
                }

                va.0 += PGSIZE;
                pa += PGSIZE;
            } else {
                // Allocation error
                return Err(());
            }
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct Kvm(PageTable);

impl Kvm {
    fn new() -> Result<Self, core::alloc::AllocError> {
        Ok(Self(PageTable::new()?))
    }

    pub fn map(&mut self, va: VA, pa: PA, size: usize, perm: usize) {
        if self.0.map_pages(va, pa, size, perm).is_err() {
            panic!("kvmmap");
        }
    }

    unsafe fn make(&mut self) {
        // uart registers
        self.map(VA(UART0), PA(UART0), PGSIZE, PTE_R | PTE_W);

        // virtio mmio disk interface
        self.map(VA(VIRTIO0), PA(VIRTIO0), PGSIZE, PTE_R | PTE_W);

        // PLIC
        self.map(VA(PLIC), PA(PLIC), 0x40_0000, PTE_R | PTE_W);

        // kernel text executable and read-only
        self.map(
            VA(KERNBASE),
            PA(KERNBASE),
            (etext as usize) - KERNBASE,
            PTE_R | PTE_X,
        );

        // kernel data and the physical RAM
        self.map(
            VA(etext as usize),
            PA(etext as usize),
            PHYSTOP - (etext as usize),
            PTE_R | PTE_W,
        );

        // trampoline for trap entry/exit mapped to the highest virtual address in the kernel
        self.map(
            VA(TRAMPOLINE),
            PA(trampoline as usize),
            PGSIZE,
            PTE_R | PTE_X,
        );

        PROCS.map_stacks();
    }
}

// Initialize kernel page table
pub fn kinit() {
    unsafe {
        KVM.initialize(Kvm::new);
        KVM.get_mut().expect("kvm to be init").make();
    }
}

// Switch hardware page table register to the kernel's page table and enable paging
pub fn init_hart() {
    unsafe {
        // wait for any previous writes to the page table memory to finish
        vma::sfence();

        // set kvm as the page table address
        satp::write(satp::make(KVM.get().unwrap().0.as_pa().0));

        // flush stale entries from the TLB
        vma::sfence();
    }
}
