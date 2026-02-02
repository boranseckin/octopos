#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
    write(1, b"sh: hello from userspace!\n");
    loop {}
}
