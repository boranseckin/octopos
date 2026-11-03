#![no_std]
#![no_main]

use user::prelude::*;

use user::io::{close, pipe, read, write};
use user::process::{fork, getpid, kill, sleep, wait};
use user::random::random;
use user::syscall::raw;

/// getpid must return a valid (non-zero) pid, and a forked child must receive a
/// different pid from its parent.
fn test_getpid() {
    let parent_pid = getpid();
    assert!(parent_pid > 0, "parent pid must be non-zero");

    if fork().expect("fork") == 0 {
        let child_pid = getpid();
        // Communicate the result through the exit code: exit(0) = pid was different.
        exit(if child_pid != parent_pid { 0 } else { 1 });
    }

    let mut code = 0;
    wait(&mut code).expect("wait");
    assert_eq!(code, 0, "child reported same pid as parent");
}

/// wait() with no living children must return `NoChildren`.
fn test_wait_no_children() {
    assert_eq!(
        wait(&mut 0),
        Err(SysError::NoChildren),
        "wait with no children must fail"
    );
}

/// Fork several children with known exit codes, collect them all with wait, and
/// verify that each code is received exactly once. This exercises multi-child
/// reaping regardless of scheduling order.
fn test_multiple_children() {
    const N: usize = 4;

    for i in 0..N {
        if fork().expect("fork") == 0 {
            exit(i + 1); // exit codes 1..=N
        }
    }

    // Collect N children; track which exit codes we've seen.
    let mut seen = [false; N + 1]; // index 0 unused; 1..=N are valid codes
    for _ in 0..N {
        let mut code = 0;
        wait(&mut code).expect("wait");
        assert!((1..=N).contains(&code), "unexpected exit code {}", code);
        assert!(!seen[code], "duplicate exit code {}", code);
        seen[code] = true;
    }

    for (i, received) in seen.iter().enumerate().skip(1) {
        assert!(received, "exit code {} was never received", i);
    }
}

/// Killing a child process must cause wait to return for that child.
/// The child sleeps in a loop; the parent kills it after a brief yield, then waits.
fn test_kill() {
    let (read_fd, write_fd) = pipe().expect("pipe");

    let pid = fork().expect("fork");
    if pid == 0 {
        // The child signals readiness by writing a byte, then sleeps indefinitely.
        close(read_fd).expect("child close read");
        write(write_fd, b"r").expect("child signal ready");
        close(write_fd).expect("child close write");
        loop {
            sleep(100).ok(); // sleeps until killed
        }
    }

    // Parent waits for the ready signal before killing.
    close(write_fd).expect("parent close write");
    let mut buf = [0; 1];
    read(read_fd, &mut buf).expect("parent read ready signal");
    close(read_fd).expect("parent close read");

    kill(pid).expect("kill");

    // wait must return and reap the killed child.
    let mut code = 0;
    let reaped = wait(&mut code).expect("wait after kill");
    assert_eq!(reaped, pid, "reaped wrong pid");
    // A killed process exits with a -1 status.
    assert_eq!(code as isize, -1, "killed child must have -1 exit code");
}

/// kill on a pid that does not exist must return an error.
fn test_kill_invalid_pid() {
    assert_eq!(
        kill(0),
        Err(SysError::NoProcess),
        "kill with invalid pid must fail"
    );
}

/// A process can wait for a grandchild via its direct child only. Once the child
/// exits, the parent's wait returns and the grandchild is re-parented to init.
/// This verifies that wait does not block on processes it did not fork.
fn test_wait_only_own_children() {
    let pid = fork().expect("fork");
    if pid == 0 {
        // Grandchild: exit immediately.
        let grandchild = fork().expect("grandchild fork");
        if grandchild == 0 {
            exit(0);
        }
        // Child: do NOT wait for the grandchild; exit straight away.
        // The grandchild will be re-parented to init.
        exit(0);
    }

    // Parent: wait for the direct child only.
    let mut code = 0;
    let reaped = wait(&mut code).expect("wait");
    assert_eq!(reaped, pid, "reaped wrong pid");
    assert_eq!(code, 0, "child exit code");

    // A second wait must fail because the parent has no more direct children.
    assert_eq!(
        wait(&mut 0),
        Err(SysError::NoChildren),
        "wait with no children must fail"
    );
}

/// random fills every byte of a buffer that spans several of the kernel's 256-byte chunks.
fn test_random_spans_chunks() {
    const LEN: usize = 1000; // three full chunks and a partial one

    let mut buf = [0u8; LEN];
    random(&mut buf).expect("random");

    // a 64-byte run of zeros from a working RNG is vanishingly unlikely
    for (i, block) in buf.chunks(64).enumerate() {
        assert!(
            block.iter().any(|&b| b != 0),
            "bytes {}..{} were never filled",
            i * 64,
            i * 64 + block.len()
        );
    }
}

/// Two calls give different bytes, so random isn't returning a fixed buffer.
fn test_random_differs() {
    let mut a = [0u8; 32];
    let mut b = [0u8; 32];
    random(&mut a).expect("random");
    random(&mut b).expect("random");
    assert_ne!(a, b, "two random buffers must differ");
}

/// A huge length at an invalid address fails cleanly instead of allocating for it.
/// Before chunking, this panicked the kernel.
fn test_random_huge_length() {
    // user programs are linked at 0, so a low address would be our own code. MAXVA (1 << 38) is
    // past the end of every user address space, so the kernel rejects it before writing.
    const PAST_MAXVA: usize = 1 << 38;
    let ret = raw::random(PAST_MAXVA as *mut u8, 1 << 40);
    assert!(ret < 0, "random into an unmapped address must fail");
    assert_eq!(
        SysError::from_code((-ret) as u16),
        SysError::BadAddress,
        "error for an unmapped address"
    );

    // and the kernel is still fine
    let mut buf = [0u8; 16];
    random(&mut buf).expect("random after a failed call");
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_getpid();
    test_wait_no_children();
    test_multiple_children();
    test_kill();
    test_kill_invalid_pid();
    test_wait_only_own_children();
    test_random_spans_chunks();
    test_random_differs();
    test_random_huge_length();
}
