/*
 * kvmmake
 * kvminit
 * kvminithart
 * walk
 * walkaddr
 * kvmmap
 * mappages
 * uvmunmap
 * uvmcreate
 * uvmfirst
 * uvmalloc
 * uvmdealloc
 * freewalk
 * uvmfree
 * uvmcopy
 * uvmclear
 * copyout
 * copyin
 * copyinstr
 */

use alloc::boxed::Box;
use alloc::vec;
use core::mem::MaybeUninit;
use core::ops::{Add, Index, IndexMut, Sub};

use crate::println;
use crate::riscv::{pa_to_pte, pg_round_down, pte_to_pa, px, MAXVA, PGSHIFT, PGSIZE, PTE_V};

#[repr(transparent)]
struct PA(usize);

#[repr(transparent)]
struct VA(usize);

#[repr(C, align(4096))]
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
struct PageTable {
    ptr: *mut RawPageTable,
}

impl PageTable {
    fn new() -> Result<Self, core::alloc::AllocError> {
        Ok(Self {
            ptr: RawPageTable::try_new()?,
        })
    }

    fn from_pa(pa: PA) -> Self {
        Self {
            ptr: pa.0 as *mut RawPageTable,
        }
    }
}

fn walk(pagetable: &mut PageTable, va: VA, alloc: bool) -> Option<&mut PageTableEntry> {
    assert!(va.0 < MAXVA, "walk");

    let mut pagetable = pagetable.ptr;

    unsafe {
        for level in (0..=2).rev() {
            let pte = (*pagetable).0.get_mut(px(level, va.0))?;

            if pte.is_v() {
                pagetable = pte.as_pa().0 as *mut RawPageTable;
            } else {
                if !alloc {
                    return None;
                }

                pagetable = RawPageTable::try_new().ok()?;
                pte.0 = pa_to_pte(pagetable as usize) | PTE_V;
            }
        }

        (*pagetable).0.get_mut(px(0, va.0))
    }
}

pub fn kinit() {}
