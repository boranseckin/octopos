#![no_std]
#![no_main]

use user::prelude::*;

use user::process::poweroff;

#[unsafe(no_mangle)]
fn main(args: Args) {
    let code = args
        .get_str(1)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);

    poweroff(code);
}
