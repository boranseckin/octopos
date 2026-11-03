#![no_std]
#![no_main]

use user::prelude::*;

use user::process::uptime;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    println!("{}", uptime());
}
