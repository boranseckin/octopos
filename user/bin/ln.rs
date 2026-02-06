#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() < 2 {
        exit_with_msg("usage: ln old new");
    }

    let old = args.get_str(1).expect("old to be str");
    let new = args.get_str(2).expect("new to be str");

    if let Err(e) = link(old, new) {
        eprintln!("ln: {} -> {}: {}", old, new, e);
    }
}
