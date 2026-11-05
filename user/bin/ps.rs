#![no_std]
#![no_main]

use user::prelude::*;

use user::process::{ProcState, proc_get};

const PID_W: usize = 5;
const STATE_W: usize = 8; // "runnable", "sleeping"
const SIZE_W: usize = 8; // KiB; 8 digits covers ~95 GiB of lazy sbrk

#[unsafe(no_mangle)]
fn main(_args: &Args) {
    println!(
        "{:>PID_W$} {:>PID_W$} {:<STATE_W$} {:>SIZE_W$} {}",
        "PID", "PPID", "STATE", "SIZE(K)", "NAME"
    );

    for i in 0.. {
        match proc_get(i) {
            Ok(Some(p)) => {
                let state = match p.state {
                    ProcState::Unused => "unused",
                    ProcState::Used => "used",
                    ProcState::Sleeping => "sleeping",
                    ProcState::Runnable => "runnable",
                    ProcState::Running => "running",
                    ProcState::Zombie => "zombie",
                };

                println!(
                    "{:>PID_W$} {:>PID_W$} {:<STATE_W$} {:>SIZE_W$} {}",
                    p.pid,
                    p.ppid,
                    state,
                    p.size / 1024,
                    p.name,
                )
            }

            Ok(None) => {}

            Err(SysError::NoEntry) => break,

            Err(e) => exit_with_msg(format!("ps: {e}")),
        }
    }
}
