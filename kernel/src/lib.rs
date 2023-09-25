#![no_std]

use spin::Mutex;
use bootloader_api::BootInfo;

use crate::frame_buffer::{ WRITER, FrameBufferWriter };

pub mod serial;
pub mod frame_buffer;

pub fn init(boot_info: &'static mut BootInfo) {
    unsafe {
        WRITER = Mutex::new(Some(FrameBufferWriter::from(boot_info)));
    }
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
