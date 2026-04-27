#![no_std]
#![no_main]

use user::*;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    if open("console", OpenFlag::READ_WRITE).is_err() {
        mknod("console", CONSOLE, 0).expect("init: cannot create console");
        open("console", OpenFlag::READ_WRITE).expect("init: cannot open console");
    }

    dup(Fd::STDIN).expect("init: dup stdout");
    dup(Fd::STDIN).expect("init: dup stderr");

    let test_mode = open("testmode", OpenFlag::READ_ONLY).map(close).is_ok();

    loop {
        let Ok(pid) = fork() else {
            exit_with_msg("init: fork failed");
        };

        if pid == 0 {
            if !test_mode {
                exec("/sh", &["sh"]);
                exit_with_msg("init: exec sh failed");
            } else {
                exec("/_testrunner", &["testrunner"]);
                exit_with_msg("init: exec testrunner failed");
            }
        }

        loop {
            // this call to wait() returns if the shell exits, or if a parentless process exits
            let wpid = wait(&mut 0);
            if let Ok(wpid) = wpid {
                if wpid == pid {
                    // shell exited; restart it
                    break;
                }
            } else {
                exit_with_msg("init: wait error");
            }
        }
    }
}
