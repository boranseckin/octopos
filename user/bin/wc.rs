#![no_std]
#![no_main]

use user::*;

fn wc(fd: Fd, name: &str) {
    let mut l = 0;
    let mut w = 0;
    let mut c = 0;
    let mut in_word = false;

    let mut buf = [0u8; 512];

    while let Ok(n) = read(fd, &mut buf) {
        if n == 0 {
            println!("{} {} {} {}", l, w, c, name);
            return;
        }

        if n < buf.len() {
            buf[n] = 0; // null-terminate the buffer for str_from_cstr
        }

        match unsafe { str_from_cstr(&buf) } {
            Ok(str) => {
                c += str.len();
                l += str.chars().filter(|&c| c == '\n').count();
                w += str.split_whitespace().count();

                if in_word && str.starts_with(|c: char| !c.is_whitespace()) {
                    w -= 1;
                }

                in_word = str.ends_with(|c: char| !c.is_whitespace());
            }
            Err(_) => {
                c += n;
                l += buf[..n].iter().filter(|&&b| b == b'\n').count();
                w += buf[..n]
                    .split(|&b| b.is_ascii_whitespace())
                    .filter(|slice| !slice.is_empty())
                    .count();

                if in_word && !buf[0].is_ascii_whitespace() {
                    w -= 1;
                }

                in_word = !buf[n - 1].is_ascii_whitespace();
            }
        }
    }

    exit_with_msg("wc: read error");
}

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() <= 1 {
        wc(Fd::STDIN, "");
        exit(0);
    }

    for name in args.args_as_str() {
        let Ok(fd) = open(name, OpenFlag::READ_ONLY) else {
            eprintln!("wc: cannot open {}", name);
            exit(1);
        };
        wc(fd, name);
        let _ = close(fd);
    }
}
