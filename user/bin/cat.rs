#![no_std]
#![no_main]

use user::*;

fn cat(fd: Fd) {
    let mut buf = [0u8; 512];

    loop {
        match read(fd, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if write(Fd::STDOUT, &buf[..n]) != Ok(n) {
                    exit_with_msg("cat: write error");
                }
            }
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
