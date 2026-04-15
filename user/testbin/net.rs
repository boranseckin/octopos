#![no_std]
#![no_main]

use user::*;

/// Host IP address (TAP interface on the host machine).
const HOST_IP: [u8; 4] = [192, 168, 10, 1];

/// Port the Python echo server in test.sh listens on.
const ECHO_SERVER_PORT: u16 = 26100;

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

/// send() must return the number of bytes written, not a truncated or wrong count.
fn test_send_returns_byte_count() {
    let fd = socket(26202).expect("socket open");

    let data = b"hello";
    let n = send(fd, data, &HOST_IP, ECHO_SERVER_PORT).expect("send");
    assert_eq!(n, data.len(), "send must return payload byte count");

    close(fd).expect("close");
}

/// Send a payload to the host echo server and verify the reflected packet arrives intact.
fn test_echo_round_trip() {
    let fd = socket(26203).expect("socket open");

    let payload = b"octopos UDP echo test";
    send(fd, payload, &HOST_IP, ECHO_SERVER_PORT).expect("send");

    let mut buf = [0u8; 64];
    let mut src_ip = [0u8; 4];
    let mut src_port = 0u16;
    let n = receive(fd, &mut buf, &mut src_ip, &mut src_port).expect("receive");

    assert_eq!(&buf[..n], payload, "echo payload mismatch");
    assert_eq!(src_ip, HOST_IP, "echo source IP must be host");
    assert_eq!(
        src_port, ECHO_SERVER_PORT,
        "echo source port must be echo server port"
    );

    close(fd).expect("close");
}

/// receive() must return the number of bytes copied (not the full payload length).
fn test_receive_truncation() {
    let fd = socket(26204).expect("socket open");

    // Send a payload larger than the receive buffer we will use.
    let payload = b"truncation test payload that is long";
    send(fd, payload, &HOST_IP, ECHO_SERVER_PORT).expect("send");

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
    test_send_returns_byte_count();
    test_echo_round_trip();
    test_receive_truncation();
}
