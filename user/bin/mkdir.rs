#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() < 2 {
        exit_with_msg("usage: mkdir directory...");
    }

    for dir in args.args_as_str() {
        if let Err(e) = mkdir(dir) {
            eprintln!("mkdir: {}: {}", dir, e);
            break;
        }
    }
}
