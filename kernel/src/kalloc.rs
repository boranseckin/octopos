use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicU8, Ordering};

use buddy_alloc::{BuddyAllocParam, buddy_alloc::BuddyAlloc};

use crate::memlayout::{KERNBASE, PHYSTOP};
use crate::riscv::PGSIZE;
use crate::spinlock::SpinLock;
use crate::vm::PA;

unsafe extern "C" {
    /// First address after kernel, defined by kernel.ld.
    static end: [u8; 0];
}

/// Reference count of each physical page.
/// The index of the array is the page number ((page address - KERNBASE) / PGSIZE).
///
/// Count is 1 when `Kmem` allocates it.
/// It is incremented when fork causes a child to share the page.
/// It is decremented each time any process drops the page from its page table.
/// Page is only deallocated when count reaches 0.
///
/// Physical memory covers pages from `KERNBASE` up to `PHYSTOP`.
/// We need one entry per page: (`PHYSTOP` - `KERNBASE`) / `PGSIZE`.
///
/// Entries from `KERNBASE` to `end` will never be touched but that's only ~56 entries.
/// To be able to allocate this array statically, we will not worry about those.
static PAGE_REFS: [AtomicU8; (PHYSTOP - KERNBASE) / PGSIZE] =
    [const { AtomicU8::new(0) }; (PHYSTOP - KERNBASE) / PGSIZE];

pub fn increment_ref(pa: PA) {
    PAGE_REFS[(pa.as_usize() - KERNBASE) / PGSIZE].fetch_add(1, Ordering::Relaxed);
}

/// Kernel memory allocator
static KMEM: Kmem = Kmem(SpinLock::new(None, "kmem"));

struct Kmem(SpinLock<Option<BuddyAlloc>>);

/// # Safety
/// Even though `BuddyAlloc` is not thread safe, `Kmem` is thread safe because it is guarded by a `SpinLock`.
unsafe impl Sync for Kmem {}

unsafe impl GlobalAlloc for Kmem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = self
            .0
            .lock()
            .as_mut()
            .expect("kmem to be init")
            .malloc(layout.size());

        // on OOM the allocator returns null; skip the ref-count update and let the caller handle it
        if !ptr.is_null() {
            PAGE_REFS[(ptr as usize - KERNBASE) / PGSIZE].store(1, Ordering::Relaxed);
        }

        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        // if the last value before decrement was 1, free the page.
        if PAGE_REFS[(ptr as usize - KERNBASE) / PGSIZE].fetch_sub(1, Ordering::Relaxed) == 1 {
            self.0.lock().as_mut().expect("kmem to be init").free(ptr)
        }
    }
}

/// Global allocator for the kernel binary. Forwards to `KMEM`.
/// Declared here but installed by main.rs, so user binaries that link this crate for `abi`
/// don't pull it in.
pub struct KernelAllocator;

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { KMEM.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe {
            KMEM.dealloc(ptr, layout);
        }
    }
}

/// Initialize kernel memory allocator.
///
/// # Safety
/// Must be called only once during kernel initialization.
pub unsafe fn init() {
    unsafe {
        println!("kmem");

        let mut guard = KMEM.0.lock();

        let size = (PHYSTOP as *const u8).offset_from(end.as_ptr()) as usize;
        let alloc_param = BuddyAllocParam::new(end.as_ptr(), size, 0x1000);
        let alloc = BuddyAlloc::new(alloc_param);

        println!("top  {:#X}", PHYSTOP);
        println!("base {:#X}", end.as_ptr() as usize);
        println!("size {:#X}\n", alloc.available_bytes());

        *guard = Some(alloc);

        println!("kmem init");
    }
}
