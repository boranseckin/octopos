#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    println!("{}", uptime());
}
