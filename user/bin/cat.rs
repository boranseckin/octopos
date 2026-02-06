#![no_std]
#![no_main]

use user::*;

fn cat(fd: usize) {
    let mut buf = [0u8; 512];

    let mut n = read(fd, &mut buf);

    while n > 0 && n != usize::MAX {
        if write(1, &buf[..n]) != n {
            exit_with_msg("cat: write error")
        }

        n = read(fd, &mut buf);
    }

    if n == usize::MAX {
        exit_with_msg("cat: read error")
    }
}

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() <= 1 {
        cat(0);
        return;
    }

    for path in args.args_as_str() {
        let Ok(fd) = open(path, OpenFlag::READ_ONLY) else {
            exit_with_msg("cat: cannot open file");
        };

        cat(fd);
        close(fd);
    }
}
