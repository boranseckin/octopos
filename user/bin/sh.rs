#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    println!("sh: hello from userspace");

    static CAT: &[u8] = b"/cat\0";
    static CAT_NAME: &[u8] = b"cat\0";
    static PATH: &[u8] = b"/LICENSE\0";

    let pid = fork();
    if pid == 0 {
        exec(CAT, &[CAT_NAME.as_ptr(), PATH.as_ptr(), core::ptr::null()]);
        exit(1);
    } else {
        wait(&mut 0);
    }

    loop {}
}
