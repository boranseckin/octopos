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
use core::ops::{Add, Sub};

use crate::riscv::{pa_to_pte, pg_round_down, pte_to_pa, px, MAXVA, PGSHIFT, PGSIZE, PTE_V};

#[repr(transparent)]
pub struct PA(pub usize);

#[repr(transparent)]
pub struct VA(pub usize);

pub struct Page([u8; 4096]);

impl Page {
    fn try_new() -> Option<Self> {
        unsafe {
            if let Ok(p) = Box::<Page>::try_new_zeroed() {
                Some(MaybeUninit::<Page>::assume_init(*p))
            } else {
                None
            }
        }
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy)]
pub struct PageTableEntry(pub usize);

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
pub struct PageTable {
    pub ptr: Box<[PageTableEntry; 512]>,
}

impl PageTable {
    pub fn new() -> Self {
        Self {
            ptr: Self::new_page(),
        }
    }

    pub fn new_page() -> Box<[PageTableEntry; 512]> {
        // A little trick to keep the size of the array in the box
        // https://stackoverflow.com/a/68122278
        let page = vec![PageTableEntry(0); 512].into_boxed_slice();
        let ptr = Box::into_raw(page) as *mut [PageTableEntry; 512];
        unsafe { Box::from_raw(ptr) }
    }

    fn from_pa(pa: PA) -> Self {
        Self {
            ptr: unsafe { Box::from_raw(pa.0 as *mut _) },
        }
    }

    fn get_entry(&self, index: usize) -> Option<&PageTableEntry> {
        unsafe { (*self.ptr).get(index) }
    }
}

pub fn walk(pagetable: &mut PageTable, va: VA, alloc: bool) -> Option<PageTableEntry> {
    assert!(va.0 < MAXVA, "walk");

    for level in (0..=2).rev() {
        unsafe {
            let mut pte = *(*pagetable).get_entry(px(level, va.0))?;

            if pte.is_v() {
                pagetable.ptr = Box::from_raw(pte.as_pa().0 as *mut _);
            } else {
                if !alloc {
                    return None;
                }

                pagetable.ptr = PageTable::new_page();

                pte.0 = pa_to_pte(pagetable.ptr.as_ptr() as usize)
            }
        }
    }

    pagetable.get_entry(px(0, va.0)).copied()
}
