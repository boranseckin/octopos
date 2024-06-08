use core::alloc::{GlobalAlloc, Layout};
use core::ptr::addr_of;

use buddy_alloc::{buddy_alloc::BuddyAlloc, BuddyAllocParam};

use crate::memlayout::PHYSTOP;
use crate::println;
use crate::spinlock::Mutex;

// furst address after kernel, defined by kernel.ld
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
        let mut guard = KMEM.0.lock();

        let size = (PHYSTOP as *const u8).offset_from(end.as_ptr()) as usize;
        println!("kmem");
        println!("base {:?}", addr_of!(end));
        println!("top  {:#X}", PHYSTOP);
        println!("size {:#X}", size);
        println!();
        let buddy_param = BuddyAllocParam::new(end.as_ptr(), size, 16);

        *guard = Some(BuddyAlloc::new(buddy_param))
    }
}
