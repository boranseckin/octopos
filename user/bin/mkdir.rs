#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() < 2 {
        exit_with_msg("usage: mkdir directory...");
    }

    for dir in args.args() {
        if mkdir(dir) == usize::MAX {
            let name = unsafe { str_from_cstr(dir).expect("name to be utf8") };
            eprintln!("mkdir: failed to create {}", name);
            break;
        }
    }
}
