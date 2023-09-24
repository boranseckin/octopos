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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QemuExitCode {
    Success = 0x0,
    Failure = 0x1,
}

pub fn exit_qemu(exit_code: QemuExitCode) {
    use x86_64::instructions::port::Port;

    unsafe {
        let mut port = Port::new(0xf4);
        port.write(exit_code as u32);
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    loop {}
}
