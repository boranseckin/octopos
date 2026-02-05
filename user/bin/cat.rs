#![no_std]
#![no_main]

use user::*;

fn cat(fd: usize) {
    let mut buf = [0u8; 512];

    let mut n = read(fd, &mut buf);

    while n > 0 && n != usize::MAX {
        if write(1, &buf[..n]) != n {
            eprintln!("cat: write error");
            exit(1);
        }

        n = read(fd, &mut buf);
    }

    if n == usize::MAX {
        eprintln!("cat: read error");
        exit(1);
    }
}

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() <= 1 {
        cat(0);
        exit(0);
    }

    for i in 1..args.len() {
        let path = args.get(i).expect("cat: invalid argument");
        let fd = open(path, OpenFlag::READ_ONLY);
        if fd == usize::MAX {
            eprintln!("cat: cannot open file");
            exit(1);
        }
        cat(fd);
        close(fd);
    }
}
