#![no_std]
#![feature(allocator_api)]
#![feature(alloc_error_handler)]

use core::sync::atomic::{AtomicBool, Ordering};

extern crate alloc;

#[macro_use]
pub(crate) mod printf;
#[macro_use]
pub(crate) mod error;
pub(crate) mod buf;
pub(crate) mod console;
pub(crate) mod entry;
pub(crate) mod exec;
pub(crate) mod file;
pub(crate) mod fs;
pub(crate) mod kalloc;
pub(crate) mod kernelvec;
pub(crate) mod log;
pub(crate) mod memlayout;
pub(crate) mod param;
pub(crate) mod plic;
pub(crate) mod proc;
pub(crate) mod riscv;
pub(crate) mod sleeplock;
pub(crate) mod spinlock;
pub(crate) mod start;
pub(crate) mod swtch;
pub(crate) mod sync;
pub(crate) mod syscall;
pub(crate) mod sysfile;
pub(crate) mod sysproc;
pub(crate) mod trampoline;
pub(crate) mod trap;
pub(crate) mod uart;
pub(crate) mod virtio_disk;
pub(crate) mod vm;

pub mod abi;

static STARTED: AtomicBool = AtomicBool::new(false);

pub extern "C" fn main() -> ! {
    let cpu_id = unsafe { proc::current_id() };
    if cpu_id == 0 {
        unsafe {
            console::init();

            println!("");
            println!("octopos kernel is booting");
            println!("");

            kalloc::init();
            vm::init();
            vm::init_hart();
            proc::init();
            trap::init();
            trap::init_hart();
            plic::init();
            plic::init_hart();
            buf::init();
            virtio_disk::init();
            proc::user_init();
        }

        println!("");

        println!("hart {} is starting", cpu_id);

        STARTED.store(true, Ordering::SeqCst);
    } else {
        while !STARTED.load(Ordering::SeqCst) {
            core::hint::spin_loop()
        }

        println!("hart {} is starting", cpu_id);

        unsafe {
            vm::init_hart();
            trap::init_hart();
            plic::init_hart();
        }
    }

    unsafe { proc::scheduler() };
}

pub fn panic_handler(info: &core::panic::PanicInfo<'_>) -> ! {
    printf::panic(info)
}
