#![allow(static_mut_refs)]

use alloc::boxed::Box;
use core::cell::UnsafeCell;
use core::cmp::min;
use core::iter::Once;
use core::mem::MaybeUninit;
use core::ops::{Add, Deref, DerefMut, Index, IndexMut, Sub};
use core::slice;

use crate::error::KernelError;
use crate::memlayout::{KERNBASE, PHYSTOP, PLIC, TRAMPOLINE, TRAPFRAME, UART0, VIRTIO0};
use crate::proc::PROCS;
use crate::riscv::{
    self, MAXVA, PGSIZE, PTE_R, PTE_V, PTE_W, PTE_X, pa_to_pte, pg_round_down, pte_to_pa, px,
    registers::{satp, vma},
};
use crate::riscv::{PTE_U, pg_round_up, pte_flags};
use crate::sync::OnceLock;
use crate::trampoline::trampoline;

// kernel.ld sets this to end of kernel code
unsafe extern "C" {
    fn etext();
}

pub static mut KVM: OnceLock<Kvm> = OnceLock::new();

#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub struct PA(pub usize);

impl From<usize> for PA {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub struct VA(pub usize);

impl From<usize> for VA {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

#[repr(C, align(4096))]
#[derive(Debug, Clone)]
struct Page([u8; 4096]);

#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
struct PageTableEntry(usize);

impl PageTableEntry {
    /// Check if the PTE is valid.
    fn is_v(&self) -> bool {
        self.0 & PTE_V != 0
    }

    /// Check if the PTE is accessible by user mode instructions.
    fn is_u(&self) -> bool {
        self.0 & PTE_U != 0
    }

    /// Check if the PTE is writable.
    fn is_w(&self) -> bool {
        self.0 & PTE_W != 0
    }

    /// Return flags of the PTE (least significant 10 bits).
    fn flags(&self) -> usize {
        pte_flags(self.0)
    }

    /// Check if the PTE is a leaf (pointing to a PA).
    fn is_leaf(&self) -> bool {
        // If the PTE is a leaf, it should have at least one of the permission bits set.
        (self.0 & (PTE_X | PTE_W | PTE_R)) != 0
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
    fn try_new() -> Result<*mut Self, KernelError> {
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
    pub fn try_new() -> Result<Self, KernelError> {
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

    fn walk(&mut self, va: VA, alloc: bool) -> Result<&mut PageTableEntry, KernelError> {
        assert!(va.0 < MAXVA, "walk");

        let mut pagetable = self.ptr;

        unsafe {
            for level in (1..=2).rev() {
                let pte = (*pagetable)
                    .get_mut(px(level, va.0))
                    .expect("walk: valid pagetable");

                if pte.is_v() {
                    pagetable = pte.as_pa().0 as *mut RawPageTable;
                } else {
                    if !alloc {
                        return Err(KernelError::InvalidPage);
                    }

                    pagetable = RawPageTable::try_new()?;
                    pte.0 = pa_to_pte(pagetable as usize) | PTE_V;
                }
            }

            Ok((*pagetable).get_mut(px(0, va.0)).unwrap())
        }
    }

    // Look up a virtual address, return the physical address, or err if not mapped.
    // Can only be used to look up user pages.
    fn walk_addr(&mut self, va: VA) -> Result<PA, KernelError> {
        if va.0 > MAXVA {
            return Err(KernelError::InvalidAddress);
        }

        let pte = self.walk(va, false)?;

        if !pte.is_v() || !pte.is_u() {
            return Err(KernelError::InvalidPte);
        }

        Ok(pte.as_pa())
    }

    // Create PTEs for virtual addresses starting at va that refer to physical addresses starting
    // at pa. va and size MUST be page-aligned.
    pub fn map_pages(
        &mut self,
        va: VA,
        pa: PA,
        size: usize,
        perm: usize,
    ) -> Result<(), KernelError> {
        assert_eq!(va.0 % PGSIZE, 0, "mappages: va not aligned");
        assert_eq!(size % PGSIZE, 0, "mappages: size not aligned");

        assert_ne!(size, 0, "map_pages: size");

        let last = va.0 + size - PGSIZE;
        let mut va = va;
        let mut pa = pa.0;

        loop {
            let pte = self.walk(va, true)?;
            assert!(!pte.is_v(), "map_pages: remap");

            pte.0 = pa_to_pte(pa) | perm | PTE_V;

            if va.0 == last {
                break;
            }

            va.0 += PGSIZE;
            pa += PGSIZE;
        }

        Ok(())
    }

    /// Recursively free page-table pages.
    /// All leaf mapping must already have been removed.
    pub fn free_walk(self) {
        let pagetable = unsafe { &mut *self.ptr };

        // iterate over all 512 PTEs
        for pte in pagetable.iter_mut() {
            if pte.is_v() {
                // if this PTE is a leaf
                if pte.is_leaf() {
                    panic!("free_walk: leaf");
                }

                // if this PTE points to a lower-level page tabel
                let child = pte.as_pa();
                let mut child = PageTable::from_pa(child);
                child.free_walk();
                *pte = PageTableEntry(0);
            }
        }

        // Free pagetable
        let _pt = unsafe { Box::from_raw(self.ptr) };
    }
}

#[derive(Debug)]
pub struct Kvm(PageTable);

impl Kvm {
    fn new() -> Result<Self, KernelError> {
        Ok(Self(PageTable::try_new()?))
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
            (etext as *const () as usize) - KERNBASE,
            PTE_R | PTE_X,
        );

        // kernel data and the physical RAM
        self.map(
            VA(etext as *const () as usize),
            PA(etext as *const () as usize),
            PHYSTOP - (etext as *const () as usize),
            PTE_R | PTE_W,
        );

        // trampoline for trap entry/exit mapped to the highest virtual address in the kernel
        self.map(
            VA(TRAMPOLINE),
            PA(trampoline as *const () as usize),
            PGSIZE,
            PTE_R | PTE_X,
        );

        unsafe { PROCS.map_stacks() };
    }
}

/// User Page Table
#[derive(Debug)]
pub struct Uvm(pub PageTable);

impl Uvm {
    /// Create an empty user page table.
    pub fn try_new() -> Result<Self, KernelError> {
        Ok(Self(PageTable::try_new()?))
    }

    /// Remove npages of mappings starting from `va`.
    /// `va` must be page-aligned and the mapping must exist.
    /// Optionally, free the physical memory.
    pub fn unmap(&mut self, va: VA, npages: usize, free: bool) {
        assert!(va.0.is_multiple_of(PGSIZE), "uvmunmap: not aligned");

        for i in (va.0..va.0 + (npages * PGSIZE)).step_by(PGSIZE) {
            match self.0.walk(va, false) {
                Err(_) => panic!("uvmunmap: walk"),
                Ok(pte) if !pte.is_v() => panic!("uvmunmap: not mapped"),
                Ok(pte) if !pte.is_leaf() => panic!("uvmunmap: not a leaf"),
                Ok(pte) => {
                    if free {
                        let pa = pte.as_pa();
                        // free page
                        let _pa = unsafe { Box::from_raw(pa.0 as *mut Page) };
                    }
                    *pte = PageTableEntry(0);
                }
            }
        }
    }

    /// Allocate PTEs and physical memory to grow process from `old_size` to `new_size`,
    /// which need not be page aligned.
    /// Returns the new process size or error.
    pub fn alloc(
        &mut self,
        old_size: usize,
        new_size: usize,
        xperm: usize,
    ) -> Result<usize, KernelError> {
        if new_size < old_size {
            return Ok(old_size);
        }

        let old_size = pg_round_up(old_size);
        for i in (old_size..new_size).step_by(PGSIZE) {
            let mem = match Box::<Page>::try_new_zeroed() {
                Ok(mem) => unsafe { mem.assume_init() },
                Err(err) => {
                    self.dealloc(i, old_size);
                    return Err(err.into());
                }
            };

            let mem = Box::into_raw(mem);

            if let Err(err) = self.0.map_pages(
                i.into(),
                (mem as usize).into(),
                PGSIZE,
                PTE_R | PTE_U | xperm,
            ) {
                let _pg = unsafe { Box::from_raw(mem) };
                self.dealloc(i, old_size);
                return Err(err);
            }
        }

        Ok(new_size)
    }

    /// Deallocate user pages to bring the process size from `old_size` to `new_size`.
    /// `old_size` and `new_size` need not be page-aligned, nor does `new_size` need to be less
    /// than `old_size`. `old_size` can be larger than the actual process size.
    /// Return the new process size.
    pub fn dealloc(&mut self, old_size: usize, new_size: usize) -> usize {
        if new_size >= old_size {
            return old_size;
        }

        let original_new_size = new_size;
        let old_size = pg_round_up(old_size);
        let new_size = pg_round_up(new_size);

        if new_size < old_size {
            let npages = (old_size - new_size) / PGSIZE;
            self.unmap(new_size.into(), npages, true);
        }

        original_new_size
    }

    /// Free user memory pages, then free page-table pages.
    pub fn free(mut self, size: usize) {
        if (size > 0) {
            self.unmap(0.into(), pg_round_up(size) / PGSIZE, true);
        }
        self.0.free_walk();
    }

    /// Free a process's page table, and free the physical memory it refers to.
    pub fn proc_free(mut self, size: usize) {
        self.unmap(TRAMPOLINE.into(), 1, false);
        self.unmap(TRAPFRAME.into(), 1, false);
        self.free(size);
    }

    // Copy from kernel to user.
    // Copy bytes from src to virtual address dstva in the current page table.
    pub fn copy_out(&mut self, dstva: VA, mut src: &[u8]) -> Result<(), KernelError> {
        let mut dstva = dstva.0;

        while !src.is_empty() {
            let mut va0 = pg_round_down(dstva);

            if va0 > MAXVA {
                return Err(KernelError::InvalidAddress);
            }

            let pte = self.walk(va0.into(), false)?;

            if pte.is_v() && pte.is_u() && pte.is_w() {
                return Err(KernelError::InvalidPte);
            }

            let pa0 = pte.as_pa();
            let n = min(PGSIZE - (dstva - va0), src.len());

            unsafe {
                let src_ptr = src[..n].as_ptr();
                let dst_ptr = (pa0.0 + (dstva - va0)) as *mut u8;
                core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, n);
            }

            src = &src[n..];
            dstva = va0 + PGSIZE;
        }

        Ok(())
    }

    // Copy from user to kernel.
    // Copy bytes from virtual address srcva to dst in the current page table.
    pub fn copy_in(&mut self, mut dst: &mut [u8], srcva: VA) -> Result<(), KernelError> {
        let mut srcva = srcva.0;

        while !dst.is_empty() {
            let va0 = pg_round_down(srcva);
            let pa0 = self.walk_addr(va0.into())?;

            let n = min(PGSIZE - (srcva - va0), dst.len());

            unsafe {
                let src_ptr = (pa0.0 + (srcva - va0)) as *const u8;
                let dst_ptr = dst.as_mut_ptr();
                core::ptr::copy_nonoverlapping(src_ptr, dst_ptr, n);
            }

            dst = &mut dst[n..];
            srcva = va0 + PGSIZE;
        }

        Ok(())
    }
}

impl Deref for Uvm {
    type Target = PageTable;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Uvm {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
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
