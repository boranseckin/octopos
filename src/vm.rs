use alloc::boxed::Box;

use core::cmp::min;
use core::mem::MaybeUninit;
use core::ops::*;
use core::ptr::{self, NonNull};

use crate::error::KernelError;
use crate::memlayout::{KERNBASE, PHYSTOP, PLIC, TRAMPOLINE, TRAPFRAME, UART0, VIRTIO0};
use crate::println;
use crate::proc::PROC_POOL;
use crate::riscv::{
    MAXVA, PGSIZE, PTE_R, PTE_U, PTE_V, PTE_W, PTE_X, pa_to_pte, pg_round_down, pg_round_up,
    pte_flags, pte_to_pa, px,
    registers::{satp, vma},
};
use crate::sync::OnceLock;
use crate::trampoline::trampoline;

// kernel.ld sets this to end of kernel code
unsafe extern "C" {
    fn etext();
}

macro_rules! impl_ops {
    ($target:ident, $trait:ident, $func:ident, $trait_assign:ident, $func_assign:ident) => {
        impl core::ops::$trait for $target {
            type Output = Self;
            #[inline]
            fn $func(self, rhs: Self) -> Self::Output {
                Self(self.0.$func(rhs.0))
            }
        }

        impl core::ops::$trait<usize> for $target {
            type Output = Self;
            #[inline]
            fn $func(self, rhs: usize) -> Self::Output {
                Self(self.0.$func(rhs))
            }
        }

        impl core::ops::$trait_assign for $target {
            #[inline]
            fn $func_assign(&mut self, rhs: Self) {
                self.0.$func_assign(rhs.0);
            }
        }

        impl core::ops::$trait_assign<usize> for $target {
            #[inline]
            fn $func_assign(&mut self, rhs: usize) {
                self.0.$func_assign(rhs);
            }
        }
    };
}

macro_rules! impl_cmp {
    ($target:ident) => {
        impl core::cmp::PartialEq<usize> for $target {
            fn eq(&self, other: &usize) -> bool {
                self.0.eq(other)
            }
        }

        impl core::cmp::PartialEq<$target> for usize {
            fn eq(&self, other: &$target) -> bool {
                self.eq(&other.0)
            }
        }

        impl core::cmp::PartialOrd<usize> for $target {
            fn partial_cmp(&self, other: &usize) -> core::option::Option<core::cmp::Ordering> {
                self.0.partial_cmp(other)
            }
        }

        impl core::cmp::PartialOrd<$target> for usize {
            fn partial_cmp(&self, other: &$target) -> core::option::Option<core::cmp::Ordering> {
                self.partial_cmp(&other.0)
            }
        }
    };
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PA(usize);

impl PA {
    /// Creates a new PA.
    pub const fn new(address: usize) -> Self {
        Self(address)
    }

    /// Returns the underlying usize value of the PA.
    pub fn as_usize(&self) -> usize {
        self.0
    }

    /// Returns the PA as a mutable pointer of type T.
    pub fn as_mut_ptr<T>(&self) -> *mut T {
        self.as_usize() as *mut T
    }

    /// Returns the PA as a PageTableEntry.
    fn as_pte(&self) -> PageTableEntry {
        PageTableEntry::from(*self)
    }
}

impl From<usize> for PA {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl_ops!(PA, Add, add, AddAssign, add_assign);
impl_ops!(PA, Sub, sub, SubAssign, sub_assign);
impl_ops!(PA, Rem, rem, RemAssign, rem_assign);
impl_ops!(PA, BitAnd, bitand, BitAndAssign, bitand_assign);
impl_ops!(PA, BitOr, bitor, BitOrAssign, bitor_assign);
impl_ops!(PA, BitXor, bitxor, BitXorAssign, bitxor_assign);
impl_cmp!(PA);

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VA(usize);

impl VA {
    /// Creates a new VA.
    pub const fn new(address: usize) -> Self {
        Self(address)
    }

    /// Returns the underlying usize value of the VA.
    pub fn as_usize(&self) -> usize {
        self.0
    }

    /// Returns the VA as a mutable pointer of type T.
    pub fn as_mut_ptr<T>(&self) -> *mut T {
        self.as_usize() as *mut T
    }

    /// Returns the page table index for the given level.
    fn px(&self, level: usize) -> usize {
        px(level, self.as_usize())
    }
}

impl From<usize> for VA {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl_ops!(VA, Add, add, AddAssign, add_assign);
impl_ops!(VA, Sub, sub, SubAssign, sub_assign);
impl_ops!(VA, Rem, rem, RemAssign, rem_assign);
impl_ops!(VA, BitAnd, bitand, BitAndAssign, bitand_assign);
impl_ops!(VA, BitOr, bitor, BitOrAssign, bitor_assign);
impl_ops!(VA, BitXor, bitxor, BitXorAssign, bitxor_assign);
impl_cmp!(VA);

#[repr(C, align(4096))]
#[derive(Debug, Clone)]
struct Page([u8; 4096]);

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PageTableEntry(usize);

impl PageTableEntry {
    /// Check if the PTE is valid.
    fn is_v(&self) -> bool {
        *self & PTE_V != 0
    }

    /// Check if the PTE is accessible by user mode instructions.
    fn is_u(&self) -> bool {
        *self & PTE_U != 0
    }

    /// Check if the PTE is writable.
    fn is_w(&self) -> bool {
        *self & PTE_W != 0
    }

    /// Return flags of the PTE (least significant 10 bits).
    fn flags(&self) -> usize {
        pte_flags(self.as_usize())
    }

    /// Check if the PTE is a leaf (pointing to a PA).
    fn is_leaf(&self) -> bool {
        // If the PTE is a leaf, it should have at least one of the permission bits set.
        (*self & (PTE_X | PTE_W | PTE_R)) != 0
    }

    /// Returns the underlying usize value of the PTE.
    fn as_usize(&self) -> usize {
        self.0
    }

    /// Returns the PA that this PTE points to.
    fn as_pa(&self) -> PA {
        PA::from(pte_to_pa(self.0))
    }
}

impl From<PA> for PageTableEntry {
    fn from(value: PA) -> Self {
        Self(pa_to_pte(value.as_usize()))
    }
}

impl_ops!(PageTableEntry, BitAnd, bitand, BitAndAssign, bitand_assign);
impl_ops!(PageTableEntry, BitOr, bitor, BitOrAssign, bitor_assign);
impl_ops!(PageTableEntry, BitXor, bitxor, BitXorAssign, bitxor_assign);
impl_cmp!(PageTableEntry);

/// Raw Page Table structure, used by `PageTable`.
#[repr(C, align(4096))]
#[derive(Debug, Clone)]
struct RawPageTable([PageTableEntry; 512]);

impl RawPageTable {
    /// Allocates a new zeroed RawPageTable.
    ///
    /// Returns a NonNull pointer to the allocated RawPageTable on success, or a KernelError if
    /// allocation fails.
    ///
    /// The caller is responsible for freeing the allocated memory.
    fn try_new() -> Result<NonNull<Self>, KernelError> {
        let memory: Box<MaybeUninit<RawPageTable>> = Box::try_new_zeroed()?;
        let memory = unsafe { memory.assume_init() };
        Ok(NonNull::new(Box::into_raw(memory)).unwrap())
    }
}

impl core::ops::Deref for RawPageTable {
    type Target = [PageTableEntry; 512];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl core::ops::DerefMut for RawPageTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl core::ops::Index<usize> for RawPageTable {
    type Output = PageTableEntry;
    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl core::ops::IndexMut<usize> for RawPageTable {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

#[derive(Debug, Clone)]
pub struct PageTable {
    ptr: NonNull<RawPageTable>,
}

impl PageTable {
    /// Creates an empty page table.
    ///
    /// Returns a Result containing the new PageTable on success, or a KernelError if allocation
    /// fails.
    ///
    /// PageTable is not dropped automatically. Call `free_walk` to free page-table pages.
    pub fn try_new() -> Result<Self, KernelError> {
        Ok(Self {
            ptr: RawPageTable::try_new()?,
        })
    }

    /// Casts a physical address as a PageTable.
    ///
    /// # Safety: The caller must ensure that `pa` is a valid physical address pointing to a page table.
    unsafe fn from_pa(pa: PA) -> Self {
        Self {
            ptr: NonNull::new(pa.as_mut_ptr()).expect("physical address to be non null"),
        }
    }

    /// Returns the physical address of this page table.
    pub fn as_pa(&self) -> PA {
        PA(self.ptr.as_ptr() as usize)
    }

    /// Returns the address of the PTE in page table that corresponds to virtual address `va`.
    ///
    /// If `alloc` is true, create any required page-table pages.
    /// Otherwise, return an error if any required page-table page doesn't exist.
    fn walk(&mut self, va: VA, alloc: bool) -> Result<&mut PageTableEntry, KernelError> {
        assert!(va < MAXVA, "walk");

        let mut pagetable = self.ptr;

        unsafe {
            for level in (1..=2).rev() {
                let pte = pagetable
                    .as_mut()
                    .get_mut(va.px(level))
                    .expect("walk: valid pagetable");

                if pte.is_v() {
                    pagetable = NonNull::new(pte.as_pa().as_mut_ptr()).unwrap();
                } else {
                    if !alloc {
                        return Err(KernelError::InvalidPage);
                    }

                    pagetable = RawPageTable::try_new()?;
                    *pte = PA::from(pagetable.as_ptr() as usize).as_pte() | PTE_V;
                }
            }

            Ok(pagetable.as_mut().get_mut(va.px(0)).unwrap())
        }
    }

    /// Looks up a virtual address, return the physical address, or Error if not mapped.
    ///
    /// Can only be used to look up user pages.
    fn walk_addr(&mut self, va: VA) -> Result<PA, KernelError> {
        if va > MAXVA {
            return Err(KernelError::InvalidAddress);
        }

        let pte = self.walk(va, false)?;

        if !pte.is_v() || !pte.is_u() {
            return Err(KernelError::InvalidPte);
        }

        Ok(pte.as_pa())
    }

    /// Creates PTEs for virtual addresses starting at `va` that refer to physical addresses
    /// starting at `pa` applying the permissions given in `perm`.
    ///
    /// `va` and `size` must be page-aligned.
    pub fn map_pages(
        &mut self,
        va: VA,
        pa: PA,
        size: usize,
        perm: usize,
    ) -> Result<(), KernelError> {
        assert_eq!(va % PGSIZE, 0, "map_pages: va not aligned");
        assert_eq!(size % PGSIZE, 0, "map_pages: size not aligned");
        assert_ne!(size, 0, "map_pages: size");

        let last = va + size - PGSIZE;
        let mut va = va;
        let mut pa = pa;

        loop {
            let pte = self.walk(va, true)?;
            assert!(!pte.is_v(), "map_pages: remap");

            *pte = pa.as_pte() | perm | PTE_V;

            if va == last {
                break;
            }

            va += PGSIZE;
            pa += PGSIZE;
        }

        Ok(())
    }

    /// Recursively frees page-table pages.
    /// All leaf mapping must already have been removed.
    pub fn free_walk(mut self) {
        let pagetable = unsafe { self.ptr.as_mut() };

        // iterate over all 512 PTEs
        for pte in pagetable.iter_mut() {
            if pte.is_v() {
                // if this PTE is a leaf
                if pte.is_leaf() {
                    panic!("free_walk: leaf");
                }

                // if this PTE points to a lower-level page tabel
                let child = pte.as_pa();
                let mut child = unsafe { PageTable::from_pa(child) };
                child.free_walk();
                *pte = PageTableEntry(0);
            }
        }

        // Free pagetable
        let _pt = unsafe { Box::from_raw(self.ptr.as_mut()) };
    }
}

pub static KVM: OnceLock<Kvm> = OnceLock::new();

/// Kernel Page Table
#[derive(Debug)]
pub struct Kvm(PageTable);

impl Kvm {
    /// Allocates a new uninitialized kernel page table.
    fn try_new() -> Result<Self, KernelError> {
        Ok(Self(PageTable::try_new()?))
    }

    /// Maps [va, va+size) to [pa, pa+size) in the kernel page table.
    pub fn map(&mut self, va: VA, pa: PA, size: usize, perm: usize) {
        if self.0.map_pages(va, pa, size, perm).is_err() {
            panic!("kvmmap");
        }
    }

    /// Sets up the kernel page table by mapping the necessary kernel regions.
    unsafe fn make(&mut self) {
        // uart registers
        self.map(VA::from(UART0), PA::from(UART0), PGSIZE, PTE_R | PTE_W);

        // virtio mmio disk interface
        self.map(VA::from(VIRTIO0), PA::from(VIRTIO0), PGSIZE, PTE_R | PTE_W);

        // PLIC
        self.map(VA::from(PLIC), PA::from(PLIC), 0x40_0000, PTE_R | PTE_W);

        // kernel text executable and read-only
        self.map(
            VA::from(KERNBASE),
            PA::from(KERNBASE),
            (etext as *const () as usize) - KERNBASE,
            PTE_R | PTE_X,
        );

        // kernel data and the physical RAM
        self.map(
            VA::from(etext as *const () as usize),
            PA::from(etext as *const () as usize),
            PHYSTOP - (etext as *const () as usize),
            PTE_R | PTE_W,
        );

        // trampoline for trap entry/exit mapped to the highest virtual address in the kernel
        self.map(
            VA::from(TRAMPOLINE),
            PA::from(trampoline as *const () as usize),
            PGSIZE,
            PTE_R | PTE_X,
        );

        unsafe { PROC_POOL.map_stacks(self) };
    }
}

/// Safety: Kvm is immutable after initialization.
unsafe impl Sync for Kvm {}
unsafe impl Send for Kvm {}

/// User Page Table
#[derive(Debug)]
pub struct Uvm(pub PageTable);

impl Uvm {
    /// Allocates an empty user page table.
    pub fn try_new() -> Result<Self, KernelError> {
        Ok(Self(PageTable::try_new()?))
    }

    /// Removes npages of mappings starting from `va`.
    ///
    /// `va` must be page-aligned and the mapping must exist.
    ///
    /// Optionally, frees the physical memory.
    pub fn unmap(&mut self, va: VA, npages: usize, free: bool) {
        assert!(va.0.is_multiple_of(PGSIZE), "uvmunmap: not aligned");

        for i in (va.0..va.0 + (npages * PGSIZE)).step_by(PGSIZE) {
            match self.0.walk(VA::from(i), false) {
                Err(_) => panic!("uvmunmap: walk"),
                Ok(pte) if !pte.is_v() => panic!("uvmunmap: not mapped"),
                Ok(pte) if !pte.is_leaf() => panic!("uvmunmap: not a leaf"),
                Ok(pte) => {
                    if free {
                        let pa = pte.as_pa();
                        // free page
                        let _pa = unsafe { Box::from_raw(pa.as_mut_ptr::<Page>()) };
                    }
                    *pte = PageTableEntry(0);
                }
            }
        }
    }

    pub fn first(&mut self, src: &[u8]) {
        assert!(src.len() < PGSIZE, "uvmfirst: more than a page");

        let mem = match Box::<Page>::try_new_zeroed() {
            Ok(mem) => unsafe { mem.assume_init() },
            Err(_) => panic!("uvmfirst: out of memory"),
        };

        self.map_pages(
            VA::from(0),
            PA::from(Box::into_raw(mem) as usize),
            PGSIZE,
            PTE_W | PTE_R | PTE_X | PTE_U,
        );

        todo!("mem move")
    }

    /// Allocates PTEs and physical memory to grow process from `old_size` to `new_size`,
    /// which need not be page aligned.
    ///
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

    /// Deallocates user pages to bring the process size from `old_size` to `new_size`.
    ///
    /// `old_size` and `new_size` need not be page-aligned, nor does `new_size` need to be less
    /// than `old_size`. `old_size` can be larger than the actual process size.
    ///
    /// Returns the new process size.
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

    /// Frees user memory pages, then frees page-table pages.
    ///
    /// Underlying physical memory is dropped.
    pub fn free(mut self, size: usize) {
        if size > 0 {
            self.unmap(VA::from(0), pg_round_up(size) / PGSIZE, true);
        }
        self.0.free_walk();
    }

    /// Frees a process's page table, and frees the physical memory it refers to.
    ///
    /// Underlying physical memory is dropped.
    pub fn proc_free(mut self, size: usize) {
        self.unmap(VA::from(TRAMPOLINE), 1, false);
        self.unmap(VA::from(TRAPFRAME), 1, false);
        self.free(size);
    }

    /// Copies from kernel to user.
    /// Copies bytes from src to virtual address dstva in the current page table.
    pub fn copy_out(&mut self, dstva: VA, mut src: &[u8]) -> Result<(), KernelError> {
        let mut dstva = dstva.0;

        while !src.is_empty() {
            let va0 = pg_round_down(dstva);

            if va0 > MAXVA {
                return Err(KernelError::InvalidAddress);
            }

            let pte = self.walk(VA::from(va0), false)?;

            if !pte.is_v() || !pte.is_u() || !pte.is_w() {
                return Err(KernelError::InvalidPte);
            }

            let pa0 = pte.as_pa();
            let n = min(PGSIZE - (dstva - va0), src.len());

            unsafe {
                let src_ptr = src[..n].as_ptr();
                let dst_ptr = (pa0.0 + (dstva - va0)) as *mut u8;
                ptr::copy_nonoverlapping(src_ptr, dst_ptr, n);
            }

            src = &src[n..];
            dstva = va0 + PGSIZE;
        }

        Ok(())
    }

    /// Copies from user to kernel.
    /// Copy bytes from virtual address srcva to dst in the current page table.
    pub fn copy_in(&mut self, mut dst: &mut [u8], srcva: VA) -> Result<(), KernelError> {
        let mut srcva = srcva.0;

        while !dst.is_empty() {
            let va0 = pg_round_down(srcva);
            let pa0 = self.walk_addr(va0.into())?;

            let n = min(PGSIZE - (srcva - va0), dst.len());

            unsafe {
                let src_ptr = (pa0.0 + (srcva - va0)) as *const u8;
                let dst_ptr = dst.as_mut_ptr();
                ptr::copy_nonoverlapping(src_ptr, dst_ptr, n);
            }

            dst = &mut dst[n..];
            srcva = va0 + PGSIZE;
        }

        Ok(())
    }
}

impl core::ops::Deref for Uvm {
    type Target = PageTable;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl core::ops::DerefMut for Uvm {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Initializes the kernel page table.
///
/// Since KVM is static, the non-const initialization is done here.
pub fn init() {
    unsafe {
        KVM.initialize(|| {
            let mut kvm = Kvm::try_new()?;
            kvm.make();
            Ok::<_, KernelError>(kvm)
        });
    }

    println!("kvm  init");
}

/// Switches hardware page table register to the kernel's page table and enables paging.
pub fn init_hart() {
    unsafe {
        // wait for any previous writes to the page table memory to finish
        vma::sfence();

        // set kvm as the page table address
        satp::write(satp::make(KVM.get().unwrap().0.as_pa().as_usize()));

        // flush stale entries from the TLB
        vma::sfence();
    }
}
