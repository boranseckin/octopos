#![no_std]
#![no_main]

use user::*;

static SH: &[u8] = b"/sh\0";
static SH_NAME: &[u8] = b"sh\0";

#[unsafe(no_mangle)]
fn main(_args: Args) {
    if open(b"console\0", OpenFlag::READ_WRITE) == usize::MAX {
        mknod(b"console\0", CONSOLE, 0);
        open(b"console\0", OpenFlag::READ_WRITE);
    }

    dup(0); // stdout
    dup(0); // stderr

    loop {
        println!("init: starting");
        let pid = fork();
        if pid == usize::MAX {
            exit_with_msg("init: fork failed");
        }
        if pid == 0 {
            exec(SH, &[SH_NAME.as_ptr(), core::ptr::null()]);
            exit_with_msg("init: exec sh failed");
        }

        loop {
            // this call to wait() returns if the shell exits, or if a parentless process exits
            let wpid = wait(&mut 0);
            if wpid == pid {
                // shell exited; restart it
                break;
            } else if wpid == usize::MAX {
                exit_with_msg("init: wait error");
            } else {
                // do nothing
            }
        }
    }
}
