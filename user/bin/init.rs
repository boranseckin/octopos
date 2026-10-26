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

    if !test_mode {
        match mkdir("/log") {
            Ok(_) | Err(SysError::AlreadyExists) => {
                if let Err(e) = init_dhcpc() {
                    eprintln!("init: dhcpc failed: {e}");
                }
            }
            _ => eprintln!("init: mkdir /log failed"),
        }
    }

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

fn init_dhcpc() -> Result<(), SysError> {
    for interface in interfaces()? {
        if interface.is_loopback() {
            continue;
        }

        if fork()? == 0 {
            close(Fd::STDOUT).unwrap_or_else(|e| exit_with_msg(e));
            open(
                &format!("/log/dhcpc.{}", interface.id),
                OpenFlag::CREATE | OpenFlag::WRITE_ONLY | OpenFlag::TRUNCATE,
            )
            .unwrap_or_else(|e| exit_with_msg(e));
            exec("/dhcpc", &["dhcpc", interface.name()]);
            exit_with_msg("init: exec dhcpc failed");
        }
    }

    Ok(())
}
