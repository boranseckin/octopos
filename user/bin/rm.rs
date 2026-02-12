#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() < 2 {
        exit_with_msg("usage: rm files...");
    }

    for name in args.args_as_str() {
        if let Err(e) = unlink(name) {
            eprintln!("rm: {} ({})", e, name);
            break;
        }
    }
}
