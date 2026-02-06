#![no_std]
#![no_main]

use core::ptr;

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
            if chdir(path.as_bytes()) == usize::MAX {
                eprintln!("sh: cannot cd to {}", path);
            }
        }
        _ => {
            let pid = fork();
            if pid == usize::MAX {
                panic!("sh: fork failed");
            } else if pid == 0 {
                exec_cmd(cmd, &args[..argc]);
                exit_with_msg("sh: exec failed");
            } else {
                wait(&mut 0);
            }
        }
    }
}

fn exec_cmd(cmd: &str, args: &[&str]) {
    let mut buf = [0u8; 256];
    let mut ptrs: [*const u8; MAXARGS + 1] = [ptr::null(); MAXARGS + 1];
    let mut offset = 0;

    for (i, arg) in args.iter().enumerate() {
        ptrs[i] = buf[offset..].as_ptr();
        buf[offset..offset + arg.len()].copy_from_slice(arg.as_bytes());
        offset += arg.len() + 1; // buf already zeroed, null is included
    }

    // path: "/cmd"
    let mut path = [0u8; 64];
    path[0] = b'/';
    path[1..1 + cmd.len()].copy_from_slice(cmd.as_bytes());

    exec(&path, &ptrs[..args.len() + 1]);
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    // ensure that three file dsescriptors are open
    loop {
        let fd = open("console", OpenFlag::READ_ONLY).expect("sh: cannot open console");
        if fd >= 3 {
            close(fd);
            break;
        }
    }

    let mut buf = [0u8; MAXLINE];

    loop {
        write(2, b"$ ");
        let Some(line) = gets(&mut buf) else {
            break; // EOF
        };

        if line.is_empty() {
            continue;
        }

        run_cmd(line);
    }
}
