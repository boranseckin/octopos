use core::alloc::{GlobalAlloc, Layout};

use buddy_alloc::{BuddyAllocParam, buddy_alloc::BuddyAlloc};

use crate::memlayout::PHYSTOP;
use crate::println;
use crate::spinlock::SpinLock;

// first address after kernel, defined by kernel.ld
unsafe extern "C" {
    static end: [u8; 0];
}

#[global_allocator]
static KMEM: Kmem = Kmem(SpinLock::new(None, "kmem"));

struct Kmem(SpinLock<Option<BuddyAlloc>>);
unsafe impl Sync for Kmem {}

unsafe impl GlobalAlloc for Kmem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.0
            .lock()
            .as_mut()
            .expect("kmem to be init")
            .malloc(layout.size())
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        self.0.lock().as_mut().expect("kmem to be init").free(ptr)
    }
}

#[alloc_error_handler]
fn handle_alloc_error(layout: Layout) -> ! {
    panic!("alloc error: {:?}", layout)
}

pub fn init() {
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
