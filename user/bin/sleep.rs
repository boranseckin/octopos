#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() != 2 {
        exit_with_msg("usage: sleep ticks");
    }

    let Some(ticks) = args.args_as_str().next().and_then(|s| s.parse().ok()) else {
        exit_with_msg("sleep: invalid ticks");
    };

    if let Err(e) = sleep(ticks) {
        eprintln!("sleep: {}", e);
        exit(1);
    }
}
