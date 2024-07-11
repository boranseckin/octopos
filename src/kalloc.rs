use core::alloc::{GlobalAlloc, Layout};

use buddy_alloc::{buddy_alloc::BuddyAlloc, BuddyAllocParam};

use crate::memlayout::PHYSTOP;
use crate::println;
use crate::spinlock::Mutex;

// first address after kernel, defined by kernel.ld
extern "C" {
    static mut end: [u8; 0];
}

#[global_allocator]
pub static mut KMEM: Kmem = Kmem(Mutex::new(None, "kmem"));

pub struct Kmem(Mutex<Option<BuddyAlloc>>);
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
    }
}
