#![no_std]
#![no_main]

extern crate alloc;

use core::sync::atomic::{AtomicBool, Ordering};

use kernel::console;
use kernel::kalloc;
use kernel::plic;
use kernel::printf;
use kernel::println;
use kernel::proc::{self, CPU_POOL};
use kernel::trap;
use kernel::vm;

static STARTED: AtomicBool = AtomicBool::new(false);

#[unsafe(export_name = "main")]
extern "C" fn main() -> ! {
    let cpu_id = unsafe { CPU_POOL.current_id() };
    if cpu_id == 0 {
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

        proc::user_init();

        println!("");

        println!("hart {} is starting", cpu_id);

        STARTED.store(true, Ordering::SeqCst);
    } else {
        while !STARTED.load(Ordering::SeqCst) {
            core::hint::spin_loop()
        }

        println!("hart {} is starting", cpu_id);

        vm::init_hart();
        trap::init_hart();
        plic::init_hart();

        loop {
            core::hint::spin_loop()
        }
    }

    proc::scheduler();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    printf::panic(info)
}
