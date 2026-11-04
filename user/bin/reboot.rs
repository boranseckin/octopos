#![no_std]
#![no_main]

use user::prelude::*;

use user::process::reboot;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    reboot();
}
