#![no_std]
#![no_main]

use user::prelude::*;

use user::fs::open;
use user::io::{Stdout, close};

fn cat(mut fd: Fd) {
    let mut buf = [0u8; 512];

    loop {
        match fd.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => Stdout.write_all(&buf[..n]).expect("cat: write error"),
            Err(_) => exit_with_msg("cat: read error"),
        }
    }
}

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() <= 1 {
        cat(Fd::STDIN);
        return;
    }

    for path in args.args_as_str() {
        let Ok(fd) = open(path, OpenFlag::READ_ONLY) else {
            exit_with_msg("cat: cannot open file");
        };

        cat(fd);
        let _ = close(fd);
    }
}
