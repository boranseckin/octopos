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

/// Read the whole file at `path` into `buf`, returning the number of bytes read.
fn read_file(path: &str, buf: &mut [u8]) -> usize {
    let fd = open(path, OpenFlag::READ_ONLY).expect("open for read_file");
    let mut total = 0;
    loop {
        let n = read(fd, &mut buf[total..]).expect("read_file");
        if n == 0 {
            break;
        }
        total += n;
    }
    close(fd).expect("close read_file");
    total
}

/// Writes through an O_APPEND fd go to the end of the existing content, not offset 0.
fn test_append() {
    let fd = open("/fs_append", O_CREATE_RW).expect("create");
    write(fd, b"abc").expect("write initial");
    close(fd).expect("close");

    let fd = open("/fs_append", OpenFlag::WRITE_ONLY | OpenFlag::APPEND).expect("open append");
    write(fd, b"def").expect("append 1");
    write(fd, b"gh").expect("append 2");
    close(fd).expect("close append");

    let mut buf = [0u8; 16];
    let n = read_file("/fs_append", &mut buf);
    assert_eq!(&buf[..n], b"abcdefgh", "append content");

    unlink("/fs_append").expect("unlink");
}

/// Two independent O_APPEND opens of the same file each have their own offset; every write must
/// still land at the current end instead of overwriting the other fd's data.
fn test_append_two_fds() {
    let fd = open("/fs_append2", O_CREATE_RW).expect("create");
    close(fd).expect("close");

    let flags = OpenFlag::WRITE_ONLY | OpenFlag::APPEND;
    let a = open("/fs_append2", flags).expect("open a");
    let b = open("/fs_append2", flags).expect("open b");

    write(a, b"A1").expect("write a1");
    write(b, b"B1").expect("write b1");
    write(a, b"A2").expect("write a2");
    write(b, b"B2").expect("write b2");

    close(a).expect("close a");
    close(b).expect("close b");

    let mut buf = [0u8; 16];
    let n = read_file("/fs_append2", &mut buf);
    assert_eq!(&buf[..n], b"A1B1A2B2", "interleaved append content");

    unlink("/fs_append2").expect("unlink");
}

/// An O_APPEND fd must see growth from a non-append fd that extended the file after it was
/// opened, i.e. the end is computed at write time, not at open time.
fn test_append_after_external_growth() {
    let fd = open("/fs_append3", O_CREATE_RW).expect("create");
    write(fd, b"start").expect("write initial");

    let app = open("/fs_append3", OpenFlag::WRITE_ONLY | OpenFlag::APPEND).expect("open append");

    // Extend the file through the plain fd after the append fd was opened.
    write(fd, b"-more").expect("extend");
    close(fd).expect("close plain");

    write(app, b"-end").expect("append");
    close(app).expect("close append");

    let mut buf = [0u8; 32];
    let n = read_file("/fs_append3", &mut buf);
    assert_eq!(&buf[..n], b"start-more-end", "append after growth");

    unlink("/fs_append3").expect("unlink");
}

/// An append larger than one log transaction is split into chunks in the kernel; all of it must
/// land contiguously after the existing content.
fn test_append_large() {
    const LEN: usize = 4096;

    let fd = open("/fs_append4", O_CREATE_RW).expect("create");
    write(fd, b"xyz").expect("write initial");
    close(fd).expect("close");

    let mut data = [0u8; LEN];
    for (i, b) in data.iter_mut().enumerate() {
        *b = b'a' + (i % 26) as u8;
    }

    let fd = open("/fs_append4", OpenFlag::WRITE_ONLY | OpenFlag::APPEND).expect("open append");
    let n = write(fd, &data).expect("large append");
    assert_eq!(n, LEN, "large append byte count");

    let mut stat = Stat::default();
    fstat(fd, &mut stat).expect("fstat");
    assert_eq!(stat.size as usize, 3 + LEN, "size after large append");
    close(fd).expect("close append");

    let mut buf = [0u8; 3 + LEN];
    let n = read_file("/fs_append4", &mut buf);
    assert_eq!(n, 3 + LEN, "read back length");
    assert_eq!(&buf[..3], b"xyz", "prefix preserved");
    assert!(buf[3..] == data, "large append content");

    unlink("/fs_append4").expect("unlink");
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
    test_append();
    test_append_two_fds();
    test_append_after_external_growth();
    test_append_large();
}
