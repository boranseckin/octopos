#![no_std]
#![no_main]

use user::prelude::*;

use user::fs::{open, unlink};
use user::io::{close, dup, pipe, read, write};
use user::process::{fork, wait};

const O_CREATE_RW: usize = OpenFlag::CREATE | OpenFlag::READ_WRITE | OpenFlag::TRUNCATE;

/// dup creates a second fd that shares the same underlying file description.
/// Reads through either fd advance the shared offset, so consecutive reads from
/// the two fds produce non-overlapping slices of the file.
fn test_dup_shared_offset() {
    let fd = open("/fd_dup_off", O_CREATE_RW).expect("create");
    write(fd, b"abcdef").expect("write");
    close(fd).expect("close");

    let fd = open("/fd_dup_off", OpenFlag::READ_ONLY).expect("open");
    let fd2 = dup(fd).expect("dup");

    // Read first half through fd.
    let mut buf = [0; 3];
    let n = read(fd, &mut buf).expect("read fd");
    assert_eq!(n, 3, "first read byte count");
    assert_eq!(&buf, b"abc", "first read data");

    // fd2 shares the offset, so it picks up from where fd left off.
    let n = read(fd2, &mut buf).expect("read fd2");
    assert_eq!(n, 3, "second read byte count");
    assert_eq!(&buf, b"def", "second read data");

    close(fd).expect("close fd");
    close(fd2).expect("close fd2");
    unlink("/fd_dup_off").expect("unlink");
}

/// A duped pipe write end keeps the pipe open. The reader only sees EOF once
/// every copy of the write end has been closed.
fn test_dup_pipe_write_end() {
    let (read_fd, write_fd) = pipe().expect("pipe");
    let write_fd2 = dup(write_fd).expect("dup write end");

    // Close the original write end — pipe is still alive via write_fd2.
    close(write_fd).expect("close original write end");

    // Writing through the remaining copy must still work.
    write(write_fd2, b"hello").expect("write via dup");

    // Close the last write end — reader should now get EOF.
    close(write_fd2).expect("close dup write end");

    let mut buf = [0; 8];
    let n = read(read_fd, &mut buf).expect("read data");
    assert_eq!(n, 5, "read byte count");
    assert_eq!(&buf[..n], b"hello", "read data");

    let n = read(read_fd, &mut buf).expect("read at eof");
    assert_eq!(n, 0, "must be EOF after all write ends closed");

    close(read_fd).expect("close read end");
}

/// A forked child inherits the parent's open file descriptors and can read
/// from them without reopening.
fn test_fd_inheritance_across_fork() {
    let fd = open("/fd_inherit", O_CREATE_RW).expect("create");
    write(fd, b"inherited").expect("write");
    close(fd).expect("close");

    let fd = open("/fd_inherit", OpenFlag::READ_ONLY).expect("open");

    if fork().expect("fork") == 0 {
        // Child: read from the inherited fd without any open() call.
        let mut buf = [0; 9];
        let n = read(fd, &mut buf).expect("child read");
        let ok = n == 9 && &buf == b"inherited";
        close(fd).expect("child close");
        exit(if ok { 0 } else { 1 });
    }

    close(fd).expect("parent close");

    let mut code = 0;
    wait(&mut code).expect("wait");
    assert_eq!(code, 0, "child reported bad inherited read");

    unlink("/fd_inherit").expect("unlink");
}

/// After a fork, the child inherits the parent's file description including its
/// read offset. Reading bytes in the parent before forking moves the offset
/// forward, so the child picks up from exactly that position rather than from
/// the start of the file.
fn test_fork_inherited_offset() {
    let fd = open("/fd_shared_off", O_CREATE_RW).expect("create");
    write(fd, b"ABCDEF").expect("write");
    close(fd).expect("close");

    let fd = open("/fd_shared_off", OpenFlag::READ_ONLY).expect("open");

    // Advance offset to 3 in the parent before forking.
    let mut buf = [0; 3];
    let n = read(fd, &mut buf).expect("parent pre-fork read");
    assert_eq!(n, 3);
    assert_eq!(&buf, b"ABC", "parent pre-fork data");

    if fork().expect("fork") == 0 {
        // The child inherits offset=3, so it should read the second half.
        let mut buf = [0; 3];
        let n = read(fd, &mut buf).expect("child read");
        let ok = n == 3 && &buf == b"DEF";
        close(fd).expect("child close");
        exit(if ok { 0 } else { 1 });
    }

    // Parent closes its copy and waits; the child still has a reference.
    close(fd).expect("parent close");

    let mut code = 0;
    wait(&mut code).expect("wait");
    assert_eq!(code, 0, "child did not inherit correct offset");

    unlink("/fd_shared_off").expect("unlink");
}

/// Opening files past the per-process limit (NOFILE=16) must return an error.
fn test_nofile_limit() {
    const NOFILE: usize = 16;

    // Create a scratch file to open repeatedly.
    let fd = open("/fd_limit", O_CREATE_RW).expect("create scratch");
    close(fd).expect("close scratch");

    // Open until we hit the limit, collecting every fd for cleanup.
    let mut fds = [Fd::STDIN; NOFILE]; // pre-fill with a placeholder
    let mut opened = 0;

    loop {
        match open("/fd_limit", OpenFlag::READ_ONLY) {
            Ok(fd) => {
                assert!(opened < NOFILE, "opened more fds than NOFILE");
                fds[opened] = fd;
                opened += 1;
            }
            Err(e) => {
                assert_eq!(
                    e,
                    SysError::TooManyFiles,
                    "expected TooManyFiles, got {:?}",
                    e
                );
                break;
            }
        }
    }

    // 3 fds (stdin/stdout/stderr) are pre-occupied, so we expect to have
    // opened exactly NOFILE - 3 files before hitting the limit.
    assert_eq!(opened, NOFILE - 3, "must opened NOFILE-3 files");

    // After closing all opened fds, opening one more must succeed again.
    for fd in fds.iter().take(opened) {
        close(*fd).expect("cleanup close");
    }

    let fd = open("/fd_limit", OpenFlag::READ_ONLY).expect("open after cleanup must succeed");
    close(fd).expect("close after cleanup");

    unlink("/fd_limit").expect("unlink scratch");
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_dup_shared_offset();
    test_dup_pipe_write_end();
    test_fd_inheritance_across_fork();
    test_fork_inherited_offset();
    test_nofile_limit();
}
