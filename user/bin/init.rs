#![no_std]
#![no_main]

use user::*;

static SH: &str = "/sh";
static ARGV: [&str; 1] = ["sh"];

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
extern "C" fn _start() -> ! {
    if open(b"console", O_RDWR) == usize::MAX {
        mknod(b"console", CONSOLE, 0);
        open(b"console", O_RDWR);
    }

    dup(0); // stdout
    dup(0); // stderr

    loop {
        write(1, b"init: starting\n");
        let pid = fork();
        if pid == usize::MAX {
            write(1, b"init: fork failed\n");
            exit(1);
        }
        if pid == 0 {
            exec(SH, &ARGV);
            write(1, b"init: exec sh failed\n");
            exit(1);
        }

        loop {
            // this call to wait() returns if the shell exits, or if a parentless process exits
            let wpid = wait(&mut 0);
            if wpid == pid {
                // shell exited; restart it
                break;
            } else if wpid == usize::MAX {
                write(1, b"init: wait error\n");
                exit(1);
            } else {
                // do nothing
            }
        }
    }
}
