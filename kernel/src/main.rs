#![no_std]
#![no_main]

#![allow(clippy::empty_loop)]

use core::panic::PanicInfo;

use spin::Mutex;

mod frame_buffer;
use crate::frame_buffer::{WRITER, FrameBufferWriter};

bootloader_api::entry_point!(kernel_main);

fn kernel_main(boot_info: &'static mut bootloader_api::BootInfo) -> ! {
    unsafe {
        WRITER = Mutex::new(Some(FrameBufferWriter::from(boot_info)));
    }

    println!("octopOS v{}", env!("CARGO_PKG_VERSION"));
    println!("DON'T PANIC!");

    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    loop {}
}
