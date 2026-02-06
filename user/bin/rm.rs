#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() < 2 {
        exit_with_msg("Usage: rm files...");
    }

    for file in args.args() {
        if unlink(file) == usize::MAX {
            let name = unsafe { str_from_cstr(file).expect("name to be utf8") };
            eprintln!("mkdir: failed to delete {}", name);
            break;
        }
    }
}
