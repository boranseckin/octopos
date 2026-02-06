#![no_std]
#![no_main]

use user::*;

const MAXLINE: usize = 128;
const MAXARGS: usize = 16;

fn run_cmd(line: &str) {
    let mut args = [""; MAXARGS];
    let mut argc = 0;

    for part in line.split_whitespace() {
        if argc >= MAXARGS {
            break;
        }
        args[argc] = part;
        argc += 1;
    }

    if argc == 0 {
        return;
    }

    let cmd = args[0];

    match cmd {
        "exit" => exit(0),
        "cd" => {
            // chdir must happen from the parent, do not fork
            let path = if argc > 1 { args[1] } else { "/" };
            if let Err(e) = chdir(path) {
                eprintln!("sh: cd {}: {}", path, e);
            }
        }
        _ => {
            let Ok(pid) = fork() else {
                panic!("sh: fork failed");
            };

            if pid == 0 {
                exec_cmd(cmd, &args[..argc]);
                exit_with_msg("sh: exec failed");
            } else {
                let _ = wait(&mut 0);
            }
        }
    }
}

fn exec_cmd(cmd: &str, args: &[&str]) {
    // Build path: "/cmd"
    let mut path_buf = [0u8; 64];
    path_buf[0] = b'/';
    path_buf[1..1 + cmd.len()].copy_from_slice(cmd.as_bytes());
    let path_str = unsafe { core::str::from_utf8_unchecked(&path_buf[..1 + cmd.len()]) };

    exec(path_str, args);
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    // ensure that three file descriptors are open
    loop {
        let Ok(fd) = open("console", OpenFlag::READ_WRITE) else {
            exit_with_msg("sh: cannot open console");
        };
        if fd.as_raw() >= 3 {
            let _ = close(fd);
            break;
        }
    }

    let mut buf = [0u8; MAXLINE];

    loop {
        let _ = write(Fd::STDERR, b"$ ");

        let Some(line) = gets(&mut buf) else {
            break; // EOF
        };

        if line.is_empty() {
            continue;
        }

        run_cmd(line);
    }
}
