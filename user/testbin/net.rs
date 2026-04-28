#![no_std]
#![no_main]

use user::*;

/// Opening a socket succeeds, and the same port can be reused after closing.
fn test_socket_open_close() {
    let fd = socket(26200).expect("socket open");
    close(fd).expect("socket close");

    let fd = socket(26200).expect("socket reopen after close");
    close(fd).expect("socket close again");
}

/// Opening a socket on a port that is already bound must fail.
fn test_socket_duplicate_port() {
    let fd = socket(26201).expect("socket open");

    assert_eq!(
        socket(26201),
        Err(SysError::AlreadyExists),
        "duplicate port must return AlreadyExists"
    );

    close(fd).expect("close");
}

fn test_ephemeral_port() {
    let fd1 = socket(0).expect("socket open");
    let port1 = ioctl(fd1, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port");
    close(fd1).expect("close");

    let fd2 = socket(0).expect("socket open");
    let port2 = ioctl(fd2, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port");
    close(fd2).expect("close");

    assert_ne!(
        port1, port2,
        "ephemeral ports must be different across socket instances"
    );
}

/// send() must return the number of bytes written, not a truncated or wrong count.
fn test_send_returns_byte_count() {
    let fd = socket(0).expect("socket open");
    let port = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;

    let data = b"hello";
    let n = send(fd, data, &Ipv4Addr::LOOPBACK.0, port).expect("send");
    assert_eq!(n, data.len(), "send must return payload byte count");

    close(fd).expect("close");
}

/// Send a payload to the loopback address and verify the reflected packet arrives intact.
fn test_echo_round_trip() {
    let fd = socket(0).expect("socket open");
    let port = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;

    let payload = b"octopos UDP echo test";
    send(fd, payload, &Ipv4Addr::LOOPBACK.0, port).expect("send");

    let mut buf = [0u8; 64];
    let mut src_ip = [0u8; 4];
    let mut src_port = 0u16;
    let n = receive(fd, &mut buf, &mut src_ip, &mut src_port).expect("receive");

    assert_eq!(&buf[..n], payload, "echo payload mismatch");
    assert_eq!(src_ip, Ipv4Addr::LOOPBACK.0, "echo source IP must be host");
    assert_eq!(src_port, port, "echo source port must be echo server port");

    close(fd).expect("close");
}

/// receive() must return the number of bytes copied (not the full payload length).
fn test_receive_truncation() {
    let fd = socket(0).expect("socket open");
    let port = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;

    // Send a payload larger than the receive buffer we will use.
    let payload = b"truncation test payload that is long";
    send(fd, payload, &Ipv4Addr::LOOPBACK.0, port).expect("send");

    let mut buf = [0u8; 8];
    let mut src_ip = [0u8; 4];
    let mut src_port = 0u16;
    let n = receive(fd, &mut buf, &mut src_ip, &mut src_port).expect("receive");

    assert_eq!(n, buf.len(), "receive must be clamped to buffer length");
    assert_eq!(
        &buf[..n],
        &payload[..n],
        "truncated bytes must match payload prefix"
    );

    close(fd).expect("close");
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_socket_open_close();
    test_socket_duplicate_port();
    test_ephemeral_port();
    test_send_returns_byte_count();
    test_echo_round_trip();
    test_receive_truncation();
}
