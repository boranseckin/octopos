#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
    println!("sh: hello from userspace");
    loop {}
}
