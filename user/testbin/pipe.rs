#![no_std]
#![no_main]

use user::prelude::*;

use user::io::{close, pipe, read, write};
use user::process::{fork, wait};

/// Write data into the write end of a pipe, close it, then drain the read end and verify that every
/// byte arrives intact.
fn test_basic_read_write() {
    let (read_fd, write_fd) = pipe().expect("pipe");

    let data = b"pipe test data";
    let n = write(write_fd, data).expect("write");
    assert_eq!(n, data.len(), "write byte count");
    close(write_fd).expect("close write end");

    let mut buf = [0u8; 32];
    let n = read(read_fd, &mut buf).expect("read");
    assert_eq!(n, data.len(), "read byte count");
    assert_eq!(&buf[..n], data, "pipe data mismatch");

    // After draining all data and the write end is closed, the next read must
    // return 0 (EOF) rather than blocking.
    let n = read(read_fd, &mut buf).expect("read at eof");
    assert_eq!(n, 0, "read must return EOF when write end is closed");

    close(read_fd).expect("close read end");
}

/// A read on an empty pipe whose write end is still open blocks until data arrives. We verify this
/// by writing from a child process and reading in the parent.
fn test_read_blocks_until_write() {
    let (read_fd, write_fd) = pipe().expect("pipe");

    if fork().expect("fork") == 0 {
        // Child: close the read end it inherited, then write a small message.
        close(read_fd).expect("child close read");
        write(write_fd, b"hello").expect("child write");
        close(write_fd).expect("child close write");
        exit(0);
    }

    // Parent: close the write end so EOF is detectable once child exits.
    close(write_fd).expect("parent close write");

    let mut buf = [0u8; 8];
    let n = read(read_fd, &mut buf).expect("parent read");
    assert_eq!(n, 5, "parent read byte count");
    assert_eq!(&buf[..n], b"hello", "parent read data");

    let mut code = 0;
    wait(&mut code).expect("wait");
    assert_eq!(code, 0, "child did not exit cleanly");

    close(read_fd).expect("parent close read");
}

/// Writing to a pipe whose read end has been closed must return an error (EPIPE / broken pipe).
fn test_write_to_closed_read_end() {
    let (read_fd, write_fd) = pipe().expect("pipe");
    close(read_fd).expect("close read end");

    assert_eq!(
        write(write_fd, b"data"),
        Err(SysError::BrokenPipe),
        "write to broken pipe must fail"
    );

    close(write_fd).expect("close write end");
}

/// Data flows correctly through a pipe when handed across a fork: the parent
/// writes, the child reads.
fn test_parent_writes_child_reads() {
    let (read_fd, write_fd) = pipe().expect("pipe");

    if fork().expect("fork") == 0 {
        // Child: close the write end it inherited, read, then verify.
        close(write_fd).expect("child close write");
        let mut buf = [0u8; 16];
        let n = read(read_fd, &mut buf).expect("child read");
        // Signal correctness through exit code: 0 = ok, 1 = mismatch.
        let ok = n == 7 && &buf[..n] == b"octopos";
        close(read_fd).expect("child close read");
        exit(if ok { 0 } else { 1 });
    }

    // Parent: close the read end it no longer needs, write, then reap.
    close(read_fd).expect("parent close read");
    write(write_fd, b"octopos").expect("parent write");
    close(write_fd).expect("parent close write");

    let mut code = 0;
    wait(&mut code).expect("wait");
    assert_eq!(code, 0, "child reported pipe data mismatch");
}

/// Pipes can relay larger payloads that span multiple reads. We write 512 bytes
/// in one call and drain them in a loop, checking total byte count.
fn test_large_payload() {
    let (read_fd, write_fd) = pipe().expect("pipe");

    const LEN: usize = 512;
    let payload = [0xABu8; LEN];
    let n = write(write_fd, &payload).expect("write large payload");
    assert_eq!(n, LEN, "write byte count for large payload");
    close(write_fd).expect("close write end");

    let mut total = 0;
    let mut buf = [0u8; 64];
    loop {
        let n = read(read_fd, &mut buf).expect("read chunk");
        if n == 0 {
            break;
        }
        for &b in &buf[..n] {
            assert_eq!(b, 0xAB, "payload byte corrupted");
        }
        total += n;
    }
    assert_eq!(total, LEN, "total bytes received");

    close(read_fd).expect("close read end");
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_basic_read_write();
    test_read_blocks_until_write();
    test_write_to_closed_read_end();
    test_parent_writes_child_reads();
    test_large_payload();
}
