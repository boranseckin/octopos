#![no_std]
#![no_main]

use core::str;

use user::*;

// Duration each parallel worker sleeps. Timer ticks run at ~100 Hz in xv6-style systems, so 10
// ticks ≈ 100 ms. We keep it short so the demo finishes quickly while still being measurable with
// the tick counter.
const SLEEP_TICKS: usize = 10;

// Must match the -smp value in .cargo/config.toml so we actually saturate all available hardware
// threads.
const NCPU: usize = 4;

#[unsafe(no_mangle)]
fn main(_args: Args) {
    println!("PID: {}  |  Uptime: {} ticks", getpid(), uptime());
    println!();

    demo_process_management();
    demo_pipe_ipc();
    demo_parallel_timing();

    println!("done");
}

// Demonstrates that the kernel manages multiple processes simultaneously.
//
// We fork NCPU children, record the PIDs returned by fork() in the parent, then wait for all of
// them. The children do nothing but exit immediately; the interesting part is that the kernel
// assigns each a distinct PID and schedules them across available CPUs before returning to the
// parent's wait.
fn demo_process_management() {
    println!("[1] Process Management");
    println!("    Forking {} worker processes...", NCPU);

    let mut child_pids = [0; NCPU];
    for slot in child_pids.iter_mut() {
        match fork().unwrap_or_else(|_| exit_with_msg("demo: fork failed")) {
            0 => exit(0),
            pid => *slot = pid,
        }
    }

    for (i, &pid) in child_pids.iter().enumerate() {
        println!("    Worker {}: PID {}", i + 1, pid);
    }

    let mut status = 0;
    for _ in 0..NCPU {
        wait(&mut status).expect("demo: wait failed");
    }
    println!("    All {} workers exited.", NCPU);
    println!();
}

// Demonstrates inter-process communication using a kernel pipe.
//
// A child process writes a message into the write-end of the pipe; the parent blocks on a read from
// the read-end. This round-trip exercises the pipe buffer, the blocking read path, and EOF
// signalling on close.
fn demo_pipe_ipc() {
    println!("[2] Pipe IPC");

    let (read_fd, write_fd) = pipe().unwrap_or_else(|_| exit_with_msg("demo: pipe failed"));

    match fork().unwrap_or_else(|_| exit_with_msg("demo: fork failed")) {
        0 => {
            close(read_fd).expect("demo: close failed");
            write(write_fd, b"Hello from child!").expect("demo: write failed");
            close(write_fd).expect("demo: close failed");
            exit(0);
        }
        child_pid => {
            close(write_fd).expect("demo: close failed");

            let mut buf = [0u8; 64];
            let n = read(read_fd, &mut buf).expect("demo: read failed");
            close(read_fd).expect("demo: close failed");

            let mut status = 0;
            wait(&mut status).expect("demo: wait failed");

            let msg = str::from_utf8(&buf[..n]).unwrap_or("(invalid utf-8)");
            println!(
                "    Producer PID {}  ->  {} bytes: \"{}\"",
                child_pid, n, msg
            );
        }
    }
    println!();
}

// Demonstrates true parallelism across multiple CPUs.
//
// NCPU workers each sleep for SLEEP_TICKS ticks. If the kernel correctly distributes runnable
// processes across all four CPUs, the total wall-clock time is approximately SLEEP_TICKS rather
// than NCPU * SLEEP_TICKS. The measured speedup factor should be close to the number of CPUs.
fn demo_parallel_timing() {
    println!(
        "[3] Parallel Timing ({} CPUs x {} ticks)",
        NCPU, SLEEP_TICKS
    );
    println!("    Serial estimate:  {} ticks", NCPU * SLEEP_TICKS);
    println!("    Spawning workers...");

    let start = uptime();

    for _ in 0..NCPU {
        if fork().unwrap_or_else(|_| exit_with_msg("demo: fork failed")) == 0 {
            sleep(SLEEP_TICKS).expect("demo: sleep failed");
            exit(0);
        }
    }

    let mut status = 0;
    for _ in 0..NCPU {
        wait(&mut status).expect("demo: wait failed");
    }

    let elapsed = uptime().saturating_sub(start);
    println!("    Parallel actual:  {} ticks", elapsed);
    if let Some(speedup) = (NCPU * SLEEP_TICKS).checked_div(elapsed) {
        println!("    Speedup:          ~{}x  (ideal: {}x)", speedup, NCPU);
    }
    println!();
}
