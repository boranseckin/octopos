#![no_std]
#![no_main]

use core::panic::PanicInfo;

use kernel::{ init, println, serial_println };

bootloader_api::entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    init(boot_info);

    println!("octopOS v{}", env!("CARGO_PKG_VERSION"));
    println!("DON'T PANIC!");

    serial_println!("Serial aa");

    #[allow(clippy::empty_loop)]
    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    loop {}
}
