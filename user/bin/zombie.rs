#![no_std]
#![no_main]

use user::prelude::*;

use user::process::{fork, sleep};

#[unsafe(no_mangle)]
fn main(_args: Args) {
    if fork().unwrap() > 0 {
        let _ = sleep(5);
    }
    exit(0);
}
