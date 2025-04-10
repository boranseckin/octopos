#![no_std]
#![no_main]

extern crate alloc;

use core::sync::atomic::{AtomicBool, Ordering};

use kernel::console;
use kernel::kalloc;
use kernel::plic;
use kernel::printf;
use kernel::println;
use kernel::proc::{self, Cpus};
use kernel::trap;
use kernel::vm;

static STARTED: AtomicBool = AtomicBool::new(false);

#[unsafe(export_name = "main")]
extern "C" fn main() -> ! {
    let cpu_id = unsafe { Cpus::get_id() };
    if cpu_id == 0 {
        console::init();

        println!("");
        println!("octopos kernel is booting");
        println!("");

        kalloc::init();
        vm::kinit();
        vm::init_hart();
        proc::init();
        trap::init();
        trap::init_hart();
        plic::init();
        plic::init_hart();

        println!("hart {cpu_id} is starting");
        STARTED.store(true, Ordering::SeqCst);

        loop {
            core::hint::spin_loop()
        }
    } else {
        while !STARTED.load(Ordering::SeqCst) {
            core::hint::spin_loop()
        }

        println!("hart {cpu_id} is starting");

        vm::init_hart();
        trap::init_hart();
        plic::init_hart();

        loop {
            core::hint::spin_loop()
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    printf::handle_panic(info)
}
