#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() < 2 {
        exit_with_msg("usage: kill pid...");
    }

    for pid in args.args_as_str() {
        let pid = pid.parse::<usize>().unwrap_or_else(|_| {
            exit_with_msg("kill: invalid pid");
        });
        if kill(pid).is_err() {
            eprintln!("kill: failed to kill {}", pid);
        }
    }
}
