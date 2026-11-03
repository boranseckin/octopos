#![no_std]
#![no_main]

use user::prelude::*;

use user::fs::{InodeType, Stat, fstat, link, mkdir, open, unlink};
use user::io::{close, read, write};

/// Open for both reading and writing, creating the file if it doesn't exist.
const O_CREATE_RW: usize = OpenFlag::CREATE | OpenFlag::READ_WRITE | OpenFlag::TRUNCATE;

/// Create a file, write data, close it, reopen read-only, and verify the content.
fn test_create_write_read() {
    let fd = open("/fs_rw", O_CREATE_RW).expect("create");
    let data = b"hello, octopos!";
    let n = write(fd, data).expect("write");
    assert_eq!(n, data.len(), "write byte count");
    close(fd).expect("close");

    let fd = open("/fs_rw", OpenFlag::READ_ONLY).expect("open rdonly");
    let mut buf = [0u8; 15];
    let n = read(fd, &mut buf).expect("read");
    assert_eq!(n, data.len(), "read byte count");
    assert_eq!(&buf, data, "read data mismatch");
    close(fd).expect("close");

    unlink("/fs_rw").expect("unlink");
}

/// Opening a nonexistent file without O_CREATE must return an error.
fn test_open_nonexistent() {
    assert_eq!(
        open("/fs_does_not_exist", OpenFlag::READ_ONLY),
        Err(SysError::NoEntry),
        "open nonexistent should fail"
    );
}

/// fstat on an open file must report the correct type, size, and nlink.
fn test_stat() {
    let fd = open("/fs_stat", O_CREATE_RW).expect("create");
    write(fd, b"hello").expect("write");

    let mut stat = Stat::default();
    fstat(fd, &mut stat).expect("fstat");
    assert_eq!(stat.r#type, InodeType::File, "stat type");
    assert_eq!(stat.size, 5, "stat size");
    assert_eq!(stat.nlink, 1, "stat nlink");

    close(fd).expect("close");
    unlink("/fs_stat").expect("unlink");
}

/// Reopening with O_TRUNCATE must reset the file to empty.
fn test_truncate() {
    let fd = open("/fs_trunc", O_CREATE_RW).expect("create");
    write(fd, b"some data").expect("write");
    close(fd).expect("close");

    let fd = open("/fs_trunc", OpenFlag::WRITE_ONLY | OpenFlag::TRUNCATE).expect("truncate open");
    close(fd).expect("close after truncate");

    let fd = open("/fs_trunc", OpenFlag::READ_ONLY).expect("open after truncate");
    let mut buf = [0u8; 16];
    let n = read(fd, &mut buf).expect("read");
    assert_eq!(n, 0, "file must be empty after truncate");
    close(fd).expect("close");

    unlink("/fs_trunc").expect("unlink");
}

/// Creating a hard link increments nlink. Unlinking one name leaves the inode
/// reachable through the other, with nlink decremented back to 1.
fn test_link() {
    let fd = open("/fs_link_a", O_CREATE_RW).expect("create");
    write(fd, b"linked").expect("write");
    close(fd).expect("close");

    link("/fs_link_a", "/fs_link_b").expect("link");

    // Both names should report nlink == 2.
    let fd = open("/fs_link_b", OpenFlag::READ_ONLY).expect("open link");
    let mut stat = Stat::default();
    fstat(fd, &mut stat).expect("fstat after link");
    assert_eq!(stat.nlink, 2, "nlink after link");
    close(fd).expect("close");

    // Removing one name must not destroy the data.
    unlink("/fs_link_a").expect("unlink a");

    let fd = open("/fs_link_b", OpenFlag::READ_ONLY).expect("open b after unlink a");
    let mut stat = Stat::default();
    fstat(fd, &mut stat).expect("fstat after unlink a");
    assert_eq!(stat.nlink, 1, "nlink after unlink a");
    let mut buf = [0u8; 6];
    let n = read(fd, &mut buf).expect("read via b");
    assert_eq!(n, 6, "read count");
    assert_eq!(&buf, b"linked", "data via b");
    close(fd).expect("close");

    unlink("/fs_link_b").expect("unlink b");
}

/// mkdir creates a directory inode; a file can be created inside and later removed.
fn test_mkdir() {
    mkdir("/fs_dir").expect("mkdir");

    let fd = open("/fs_dir", OpenFlag::READ_ONLY).expect("open dir");
    let mut stat = Stat::default();
    fstat(fd, &mut stat).expect("fstat dir");
    assert_eq!(stat.r#type, InodeType::Directory, "dir type");
    close(fd).expect("close dir");

    let fd = open("/fs_dir/inner", O_CREATE_RW).expect("create file inside dir");
    write(fd, b"inner").expect("write inner");
    close(fd).expect("close inner");

    unlink("/fs_dir/inner").expect("unlink inner");
    unlink("/fs_dir").expect("unlink dir");
}

/// A second read at EOF must return 0 bytes.
fn test_read_eof() {
    let fd = open("/fs_eof", O_CREATE_RW).expect("create");
    write(fd, b"eof").expect("write");
    close(fd).expect("close");

    let fd = open("/fs_eof", OpenFlag::READ_ONLY).expect("open");
    let mut buf = [0u8; 16];
    let n = read(fd, &mut buf).expect("read");
    assert_eq!(n, 3, "first read");
    let n = read(fd, &mut buf).expect("read at eof");
    assert_eq!(n, 0, "second read must be EOF");
    close(fd).expect("close");

    unlink("/fs_eof").expect("unlink");
}

/// Writing to a read-only fd and reading from a write-only fd must both fail.
fn test_open_flags() {
    let fd = open("/fs_flags", O_CREATE_RW).expect("create");
    write(fd, b"content").expect("write");
    close(fd).expect("close");

    let fd = open("/fs_flags", OpenFlag::READ_ONLY).expect("open rdonly");
    assert_eq!(
        write(fd, b"nope"),
        Err(SysError::BadDescriptor),
        "write to rdonly must fail"
    );
    close(fd).expect("close rdonly");

    let fd = open("/fs_flags", OpenFlag::WRITE_ONLY).expect("open wronly");
    let mut buf = [0u8; 8];
    assert_eq!(
        read(fd, &mut buf),
        Err(SysError::BadDescriptor),
        "read from wronly must fail"
    );
    close(fd).expect("close wronly");

    unlink("/fs_flags").expect("unlink");
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_create_write_read();
    test_open_nonexistent();
    test_stat();
    test_truncate();
    test_link();
    test_mkdir();
    test_read_eof();
    test_open_flags();
}
