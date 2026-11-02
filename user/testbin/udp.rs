#![no_std]
#![no_main]

use user::*;

const ETHERTYPE_IPV4: u16 = 0x0800;
const IPPROTO_ICMP: u8 = 1;
const IPPROTO_UDP: u8 = 17;
const ICMP_DEST_UNREACHABLE: u8 = 3;
const ICMP_PORT_UNREACHABLE: u8 = 3;

// Offsets into a frame of Ethernet + IPv4 without options.
const IP_START: usize = 14;
const IP_PROTO: usize = IP_START + 9;
const IP_SRC: usize = IP_START + 12;
const IP_DEST: usize = IP_START + 16;
const L4_START: usize = IP_START + 20;

// Offsets into an ICMP error frame.
const ICMP_QUOTE: usize = L4_START + 8;
const ICMP_QUOTE_UDP: usize = ICMP_QUOTE + 20;
/// Headers, then the quoted IPv4 header and the first 8 bytes after it.
const ICMP_ERROR_LEN: usize = ICMP_QUOTE_UDP + 8;

/// Ticks to wait before deciding an ICMP error is not coming.
const ICMP_TIMEOUT: usize = 5;

/// Size of the frame built by `raw_udp_frame`.
const RAW_FRAME_LEN: usize = L4_START + 8;

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

/// RFC 1071 internet checksum. A message with a valid checksum sums to zero.
fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    let (chunks, remainder) = data.as_chunks::<2>();
    for chunk in chunks {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    if let [byte] = remainder {
        sum += (*byte as u32) << 8;
    }

    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }

    !(sum as u16)
}

/// Opens a packet socket that sees every IPv4 frame on lo.
fn open_lo_capture() -> Fd {
    let lo = loopback().expect("loopback interface").unwrap().id;
    let fd = socket(
        SocketType::Packet,
        SockAddr::link(Some(lo), Some(ETHERTYPE_IPV4)),
    )
    .expect("packet socket open");
    ioctl(fd, Ioctl::SOCKET_SET_RECV_TIMEOUT, ICMP_TIMEOUT).expect("ioctl set receive timeout");
    fd
}

/// Returns a port with no socket bound to it.
fn unbound_port() -> u16 {
    let fd = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let port = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;
    close(fd).expect("close");
    port
}

/// Reads frames until an ICMP one arrives or the socket times out.
/// Returns whether a UDP frame was seen, and the length of the ICMP frame left in `buf`.
fn capture_icmp(fd: Fd, buf: &mut [u8]) -> (bool, Option<usize>) {
    let mut saw_udp = false;

    loop {
        match receive(fd, buf) {
            Ok((n, _)) if buf[IP_PROTO] == IPPROTO_ICMP => return (saw_udp, Some(n)),
            Ok(_) => saw_udp |= buf[IP_PROTO] == IPPROTO_UDP,
            Err(SysError::TimedOut) => return (saw_udp, None),
            Err(e) => panic!("receive: {e:?}"),
        }
    }
}

/// Asserts the datagram was seen on lo, so the check below it isn't vacuous, and got no answer.
fn assert_no_icmp(fd: Fd, msg: &str) {
    let mut buf = [0u8; 128];
    let (saw_udp, icmp) = capture_icmp(fd, &mut buf);

    assert!(saw_udp, "datagram must reach lo: {msg}");
    assert_eq!(icmp, None, "{msg}");
}

/// Builds a frame holding an empty UDP datagram, for addresses a UDP socket can't send from.
fn raw_udp_frame(src: Ipv4Addr, dest: Ipv4Addr, dest_port: u16) -> [u8; RAW_FRAME_LEN] {
    let mut frame = [0u8; RAW_FRAME_LEN];

    frame[12..14].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes());

    frame[IP_START] = 0x45; // IPv4 without options
    frame[IP_START + 2..IP_START + 4].copy_from_slice(&28u16.to_be_bytes()); // total length
    frame[IP_START + 8] = 64; // ttl
    frame[IP_PROTO] = IPPROTO_UDP;
    frame[IP_SRC..IP_SRC + 4].copy_from_slice(&src.0);
    frame[IP_DEST..IP_DEST + 4].copy_from_slice(&dest.0);

    let sum = checksum(&frame[IP_START..L4_START]);
    frame[IP_START + 10..IP_START + 12].copy_from_slice(&sum.to_be_bytes());

    frame[L4_START..L4_START + 2].copy_from_slice(&1234u16.to_be_bytes()); // source port
    frame[L4_START + 2..L4_START + 4].copy_from_slice(&dest_port.to_be_bytes());
    frame[L4_START + 4..L4_START + 6].copy_from_slice(&8u16.to_be_bytes()); // length

    frame
}

/// An unbound port answers with Port Unreachable, quoting the original datagram.
fn test_port_unreachable() {
    let capture = open_lo_capture();
    let fd = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let src_port = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;
    let dest_port = unbound_port();

    send(fd, b"x", SockAddr::inet(Ipv4Addr::LOOPBACK, dest_port)).expect("send");

    let mut buf = [0u8; 128];
    let (saw_udp, icmp) = capture_icmp(capture, &mut buf);
    assert!(saw_udp, "original datagram must be seen first");
    let n = icmp.expect("unbound port must answer with ICMP");

    assert_eq!(n, ICMP_ERROR_LEN, "error frame length");
    assert_eq!(buf[L4_START], ICMP_DEST_UNREACHABLE, "icmp type");
    assert_eq!(buf[L4_START + 1], ICMP_PORT_UNREACHABLE, "icmp code");
    assert_eq!(&buf[L4_START + 4..L4_START + 8], &[0; 4], "unused bytes");
    assert_eq!(checksum(&buf[L4_START..n]), 0, "icmp checksum");

    assert_eq!(
        &buf[IP_DEST..IP_DEST + 4],
        &Ipv4Addr::LOOPBACK.0,
        "error must go back to the sender"
    );

    assert_eq!(buf[ICMP_QUOTE] >> 4, 4, "quoted ip version");
    assert_eq!(buf[ICMP_QUOTE + 9], IPPROTO_UDP, "quoted ip protocol");
    assert_eq!(
        &buf[ICMP_QUOTE + 16..ICMP_QUOTE + 20],
        &Ipv4Addr::LOOPBACK.0,
        "quoted ip destination"
    );
    assert_eq!(
        u16::from_be_bytes([buf[ICMP_QUOTE_UDP], buf[ICMP_QUOTE_UDP + 1]]),
        src_port,
        "quoted source port"
    );
    assert_eq!(
        u16::from_be_bytes([buf[ICMP_QUOTE_UDP + 2], buf[ICMP_QUOTE_UDP + 3]]),
        dest_port,
        "quoted destination port"
    );

    close(fd).expect("close");
    close(capture).expect("close");
}

/// A bound port delivers the datagram and answers nothing.
fn test_no_port_unreachable_when_bound() {
    let capture = open_lo_capture();
    let fd = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let port = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;

    send(fd, b"x", SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("send");

    let mut buf = [0u8; 8];
    receive(fd, &mut buf).expect("bound socket must receive");
    assert_no_icmp(capture, "bound port must not answer");

    close(fd).expect("close");
    close(capture).expect("close");
}

/// A destination that is not a unicast address of this host never gets an answer.
fn test_no_port_unreachable_for_broadcast() {
    let capture = open_lo_capture();
    let fd = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let port = unbound_port();

    send(
        fd,
        b"x",
        SockAddr::inet(Ipv4Addr([127, 255, 255, 255]), port),
    )
    .expect("send");
    assert_no_icmp(capture, "subnet broadcast must not answer");

    close(fd).expect("close");
    close(capture).expect("close");
}

/// A source that does not name a single host never gets an answer.
fn test_no_port_unreachable_for_bad_source() {
    let capture = open_lo_capture();
    let lo = loopback().expect("loopback interface").unwrap().id;
    let addr = SockAddr::link(Some(lo), Some(ETHERTYPE_IPV4));
    let port = unbound_port();

    // without a route to these sources an answer fails to send, and the test would pass with no
    // guard in the kernel. the high metric keeps any real default route preferred.
    let fallback = RouteEntry {
        dest_ip: Ipv4Addr::UNSPECIFIED,
        prefix_len: 0,
        gateway: None,
        interface_id: lo,
        metric: 1000,
        owner: RouteOwner::Static,
        kind: RouteKind::Unicast,
        pref_src: None,
    };
    route_add(&fallback, 0).expect("route_add fallback");

    let sources = [
        ("unspecified", Ipv4Addr::UNSPECIFIED),
        ("broadcast", Ipv4Addr([255, 255, 255, 255])),
        ("multicast", Ipv4Addr([224, 0, 0, 1])),
    ];

    for (name, src) in sources {
        let frame = raw_udp_frame(src, Ipv4Addr::LOOPBACK, port);
        send(capture, &frame, addr).expect("send raw frame");
        assert_no_icmp(capture, name);
    }

    route_remove(&fallback).expect("route_remove fallback");
    close(capture).expect("close");
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
    test_port_unreachable();
    test_no_port_unreachable_when_bound();
    test_no_port_unreachable_for_broadcast();
    test_no_port_unreachable_for_bad_source();
}
