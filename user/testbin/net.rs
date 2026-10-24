#![no_std]
#![no_main]

use user::*;

/// Opening a socket succeeds, and the same port can be reused after closing.
fn test_socket_open_close() {
    let addr = SockAddr::any_inet_with_port(26200);

    let fd = socket(SocketType::Udp, addr).expect("socket open");
    close(fd).expect("socket close");

    let fd = socket(SocketType::Udp, addr).expect("socket reopen after close");
    close(fd).expect("socket close again");
}

/// Opening a socket on a port that is already bound must fail.
fn test_socket_duplicate_port() {
    let addr = SockAddr::any_inet_with_port(26201);

    let fd = socket(SocketType::Udp, addr).expect("socket open");

    assert_eq!(
        socket(SocketType::Udp, addr),
        Err(SysError::AlreadyExists),
        "duplicate port must return AlreadyExists"
    );

    close(fd).expect("close");
}

fn test_ephemeral_port() {
    let fd1 = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let port1 = ioctl(fd1, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port");
    close(fd1).expect("close");

    let fd2 = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let port2 = ioctl(fd2, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port");
    close(fd2).expect("close");

    assert_ne!(
        port1, port2,
        "ephemeral ports must be different across socket instances"
    );
}

/// send() must return the number of bytes written, not a truncated or wrong count.
fn test_send_returns_byte_count() {
    let fd = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let port = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;

    let data = b"hello";
    let n = send(fd, data, SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("send");
    assert_eq!(n, data.len(), "send must return payload byte count");

    close(fd).expect("close");
}

/// Send a payload to the loopback address and verify the reflected packet arrives intact.
fn test_echo_round_trip() {
    let fd = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let port = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;

    let payload = b"octopos UDP echo test";
    send(fd, payload, SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("send");

    let mut buf = [0u8; 64];
    let (n, addr) = receive(fd, &mut buf).expect("receive");
    let SockAddr::Inet {
        ip: src_ip,
        port: src_port,
    } = addr
    else {
        unreachable!()
    };

    assert_eq!(&buf[..n], payload, "echo payload mismatch");
    assert_eq!(src_ip, Ipv4Addr::LOOPBACK, "echo source IP must be host");
    assert_eq!(src_port, port, "echo source port must be echo server port");

    close(fd).expect("close");
}

/// receive() must return the number of bytes copied (not the full payload length).
fn test_receive_truncation() {
    let fd = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let port = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;

    // Send a payload larger than the receive buffer we will use.
    let payload = b"truncation test payload that is long";
    send(fd, payload, SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("send");

    let mut buf = [0u8; 8];
    let (n, _addr) = receive(fd, &mut buf).expect("receive");

    assert_eq!(n, buf.len(), "receive must be clamped to buffer length");
    assert_eq!(
        &buf[..n],
        &payload[..n],
        "truncated bytes must match payload prefix"
    );

    close(fd).expect("close");
}

fn test_add_remove_multiple() {
    let fd1 = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let fd2 = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let fd3 = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");

    let port2 = ioctl(fd2, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;
    let port3 = ioctl(fd3, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;

    close(fd1).expect("close");
    close(fd3).expect("close");

    let fd3 = socket(SocketType::Udp, SockAddr::any_inet_with_port(port3)).expect("socket open");

    let payload = b"octopos UDP echo test";
    send(fd2, payload, SockAddr::inet(Ipv4Addr::LOOPBACK, port2)).expect("send");

    let mut buf = [0u8; 64];
    let (n, addr) = receive(fd2, &mut buf).expect("receive");
    let SockAddr::Inet {
        ip: src_ip,
        port: src_port,
    } = addr
    else {
        unreachable!()
    };

    assert_eq!(&buf[..n], payload, "echo payload mismatch");
    assert_eq!(src_ip, Ipv4Addr::LOOPBACK, "echo source IP must be host");
    assert_eq!(src_port, port2, "echo source port must be echo server port");

    close(fd2).expect("close");
    close(fd3).expect("close");
}

/// Opens a UDP socket on an ephemeral port and returns it with its port.
fn open_udp() -> (Fd, u16) {
    let fd = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let port = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;
    (fd, port)
}

fn set_receive_timeout(fd: Fd, ticks: usize) {
    ioctl(fd, Ioctl::SOCKET_SET_RECV_TIMEOUT, ticks).expect("ioctl set receive timeout");
}

/// With nothing to receive, receive() gives up after the timeout, and not before it.
fn test_receive_timeout() {
    const TIMEOUT: usize = 3;

    let (fd, _) = open_udp();
    set_receive_timeout(fd, TIMEOUT);

    let start = uptime();
    let mut buf = [0u8; 64];
    assert_eq!(
        receive(fd, &mut buf).map(|(n, _)| n),
        Err(SysError::TimedOut),
        "receive with nothing queued must time out"
    );
    assert!(
        uptime() - start >= TIMEOUT,
        "receive must not time out before the deadline"
    );

    close(fd).expect("close");
}

/// The deadline is computed per call, so a second receive waits the full timeout again.
fn test_receive_timeout_per_call() {
    const TIMEOUT: usize = 2;

    let (fd, _) = open_udp();
    set_receive_timeout(fd, TIMEOUT);

    let mut buf = [0u8; 64];
    for _ in 0..2 {
        let start = uptime();
        assert_eq!(
            receive(fd, &mut buf).map(|(n, _)| n),
            Err(SysError::TimedOut),
            "receive with nothing queued must time out"
        );
        assert!(
            uptime() - start >= TIMEOUT,
            "each receive must wait the full timeout"
        );
    }

    close(fd).expect("close");
}

/// A queued datagram is returned well before a long timeout expires.
fn test_receive_before_timeout() {
    const TIMEOUT: usize = 50;

    let (fd, port) = open_udp();
    set_receive_timeout(fd, TIMEOUT);

    let payload = b"octopos UDP timeout test";
    send(fd, payload, SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("send");

    let start = uptime();
    let mut buf = [0u8; 64];
    let (n, _) = receive(fd, &mut buf).expect("receive must not time out");
    assert_eq!(&buf[..n], payload, "payload mismatch");
    assert!(
        uptime() - start < TIMEOUT,
        "receive must return as soon as the datagram arrives"
    );

    close(fd).expect("close");
}

/// Setting the timeout back to 0 restores blocking receive, which still delivers datagrams.
fn test_receive_timeout_cleared() {
    let (fd, port) = open_udp();
    set_receive_timeout(fd, 2);
    set_receive_timeout(fd, 0);

    let payload = b"octopos UDP blocking test";
    send(fd, payload, SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("send");

    let mut buf = [0u8; 64];
    let (n, _) = receive(fd, &mut buf).expect("blocking receive");
    assert_eq!(&buf[..n], payload, "payload mismatch");

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
    test_add_remove_multiple();
    test_receive_timeout();
    test_receive_timeout_per_call();
    test_receive_before_timeout();
    test_receive_timeout_cleared();
}
