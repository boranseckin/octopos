//! Every unit is rounded up to a whole number of units.
//! One unit is `size_of::<Header>()`, which is 16 bytes.
//!
//! Each block, free or allocated, starts with one header.
//! A request for `n` bytes requires `units = ceil(n / 16) + 1`.
//!
//! The pointer returned back after `alloc` is the address just after the header.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr;

use crate::sbrk;

/// Minimum number of headers to request from sbrk at once (4 KiB).
const MIN_GROW_UNITS: usize = 0x1000 / size_of::<Header>();

/// Free-list block header.
#[repr(C, align(16))]
struct Header {
    /// Next block in the free list.
    next: *mut Header,
    /// Counts the block's total length in units, including the header.
    size: usize,
}

struct FreeList {
    /// Size-0 sentinel; starts out pointing at itself.
    base: Header,
    /// Where the next search starts. Null until first use.
    freep: *mut Header,
}

pub struct Allocator(UnsafeCell<FreeList>);

// # Safety: user processes are single-threaded; fork gives the child its own copy.
unsafe impl Sync for Allocator {}

impl FreeList {
    /// First fit, starting at `freep`.
    /// Splits the tail off larger blocks.
    /// Calls `grow` after a full pass with no fit.
    unsafe fn alloc(&mut self, units: usize) -> *mut Header {
        unsafe {
            if self.base.next.is_null() {
                self.base.next = &mut self.base as *mut Header;
                self.freep = self.base.next;
            }

            let mut prev = self.freep;
            let mut p = (*prev).next;

            loop {
                if (*p).size >= units {
                    if (*p).size == units {
                        // exact fit; unlink the whole block
                        (*prev).next = (*p).next;
                    } else {
                        // block is bigger than the request; carve off the tail
                        (*p).size -= units;
                        p = p.add((*p).size);
                        // write a new header here (only size matters)
                        (*p).size = units;
                    }

                    self.freep = prev;
                    return p.add(1);
                }

                // went all the way around: time to grow
                if p == self.freep {
                    if self.grow(units).is_err() {
                        return ptr::null_mut();
                    }
                    p = self.freep;
                }

                prev = p;
                p = (*p).next;
            }
        }
    }

    /// Inserts `block` in address order and merges it with its neighbours.
    unsafe fn free(&mut self, block: *mut Header) {
        unsafe {
            let mut p = self.freep;

            while !(p < block && block < (*p).next) {
                if p >= (*p).next && (block > p || block < (*p).next) {
                    // wrap-around point
                    // p is the highest address and p.next the lowest
                    break;
                }

                p = (*p).next;
            }

            // merge with upper neighbour
            if block.add((*block).size) == (*p).next {
                (*block).size += (*(*p).next).size;
                (*block).next = (*(*p).next).next;
            } else {
                (*block).next = (*p).next;
            }

            // merge with lower neighbour
            if p.add((*p).size) == block {
                (*p).size += (*block).size;
                (*p).next = (*block).next;
            } else {
                (*p).next = block;
            }

            self.freep = p;
        }
    }

    /// Calls sbrk for max(units, MIN_GROW_UNITS) headers and frees the new block into the list.
    unsafe fn grow(&mut self, units: usize) -> Result<(), ()> {
        unsafe {
            let n = units.max(MIN_GROW_UNITS);

            let Some(bytes) = n
                .checked_mul(size_of::<Header>())
                .and_then(|b| isize::try_from(b).ok())
            else {
                return Err(());
            };

            let Ok(mem) = sbrk(bytes) else {
                return Err(());
            };

            debug_assert!(mem % 16 == 0);

            // new pages are zeroed, so size is 0.
            let header = mem as *mut Header;
            (*header).size = n;

            // add the new block to the list
            self.free(header);

            Ok(())
        }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator = Allocator(UnsafeCell::new(FreeList {
    base: Header {
        next: ptr::null_mut(),
        size: 0,
    },
    freep: ptr::null_mut(),
}));

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() > size_of::<Header>() {
            return ptr::null_mut();
        }

        let units = layout.size().div_ceil(size_of::<Header>()) + 1;

        unsafe { (*self.0.get()).alloc(units).cast::<u8>() }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        unsafe {
            // header is one unit before ptr
            let header = ptr.cast::<Header>().sub(1);
            (*self.0.get()).free(header);
        }
    }
}
