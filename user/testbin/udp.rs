#![no_std]
#![no_main]

use user::prelude::*;

use user::io::{Ioctl, close, ioctl};
use user::net::{
    InterfaceId, RouteEntry, RouteKind, RouteOwner, SocketType, interface_iter, loopback, receive,
    route_add, route_remove, send, socket,
};
use user::process::uptime;
use user::syscall::{RawSockAddr, raw};

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
    let reply = receive(fd, &mut buf).expect("receive");
    let SockAddr::Inet {
        ip: src_ip,
        port: src_port,
        interface: None,
    } = reply.src
    else {
        unreachable!()
    };

    assert_eq!(reply.data, payload, "echo payload mismatch");
    assert_eq!(src_ip, Ipv4Addr::LOOPBACK, "echo source IP must be host");
    assert_eq!(src_port, port, "echo source port must be echo server port");

    close(fd).expect("close");
}

/// receive() must set truncated when the buffer is too small to fit the datagram.
fn test_receive_truncation() {
    let fd = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let port = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;

    // Send a payload larger than the receive buffer we will use.
    let payload = b"truncation test payload that is long";
    send(fd, payload, SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("send");

    const BUF_LEN: usize = 8;
    let mut buf = [0u8; BUF_LEN];
    let reply = receive(fd, &mut buf).expect("receive");

    assert!(reply.truncated, "receive must indicate truncated");
    assert_eq!(
        reply.len(),
        BUF_LEN,
        "receive must be clamped to buffer length"
    );
    assert_eq!(
        reply.data,
        &payload[..reply.len()],
        "truncated bytes must match payload prefix"
    );

    // Send a payload with the exact size as the receive buffer we will use.
    let payload = b"12345678";
    send(fd, payload, SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("send");

    let mut buf = [0u8; BUF_LEN];
    let reply = receive(fd, &mut buf).expect("receive");

    assert!(!reply.truncated, "receive must not indicate truncated");
    assert_eq!(
        reply.len(),
        BUF_LEN,
        "receive must be clamped to buffer length"
    );
    assert_eq!(
        reply.data,
        &payload[..reply.len()],
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
    let reply = receive(fd2, &mut buf).expect("receive");
    let SockAddr::Inet {
        ip: src_ip,
        port: src_port,
        interface: None,
    } = reply.src
    else {
        unreachable!()
    };

    assert_eq!(reply.data, payload, "echo payload mismatch");
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
        receive(fd, &mut buf).map(|r| r.len()),
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
            receive(fd, &mut buf).map(|r| r.len()),
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
    let reply = receive(fd, &mut buf).expect("receive must not time out");
    assert_eq!(reply.data, payload, "payload mismatch");
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
    let reply = receive(fd, &mut buf).expect("blocking receive");
    assert_eq!(reply.data, payload, "payload mismatch");

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
            Ok(reply) if reply.data[IP_PROTO] == IPPROTO_ICMP => {
                return (saw_udp, Some(reply.len()));
            }
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

/// Converts a raw syscall return value into a result, like the safe wrappers do.
fn raw_result(ret: isize) -> Result<usize, SysError> {
    if ret >= 0 {
        Ok(ret as usize)
    } else {
        Err(SysError::from_code((-ret) as u16))
    }
}

/// A send length far larger than any buffer is rejected before the kernel allocates for it.
/// Before the limit check, this panicked the kernel and ended the whole test run.
fn test_send_rejects_huge_length() {
    let (fd, port) = open_udp();
    let dest = RawSockAddr::from(SockAddr::inet(Ipv4Addr::LOOPBACK, port));
    let buf = [0u8; 8];

    for len in [1usize << 40, usize::MAX, 1515] {
        let ret = raw::send(fd.as_raw(), buf.as_ptr(), len, &dest);
        assert_eq!(
            raw_result(ret),
            Err(SysError::MessageTooLarge),
            "send of {len} bytes"
        );
    }

    // the kernel and the socket still work
    send(fd, b"ok", SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("send after rejection");
    let mut out = [0u8; 8];
    let reply = receive(fd, &mut out).expect("receive after rejection");
    assert_eq!(&reply.data, b"ok", "payload after rejection");

    close(fd).expect("close");
}

/// A datagram to an address on lo's subnet that isn't ours is dropped, even with a socket on
/// 0.0.0.0 and the matching port.
fn test_drops_other_hosts() {
    let capture = open_lo_capture();
    let (fd, port) = open_udp();
    set_receive_timeout(fd, ICMP_TIMEOUT);

    send(fd, b"x", SockAddr::inet(Ipv4Addr([127, 0, 0, 2]), port)).expect("send");

    // the datagram did reach lo, so a timeout below means it was dropped, not never sent
    let mut frame = [0u8; 128];
    let (saw_udp, _) = capture_icmp(capture, &mut frame);
    assert!(saw_udp, "datagram must reach lo");

    let mut buf = [0u8; 8];
    assert_eq!(
        receive(fd, &mut buf).map(|r| r.len()),
        Err(SysError::TimedOut),
        "a datagram for another host must not be delivered"
    );

    close(fd).expect("close");
    close(capture).expect("close");
}

/// A datagram to lo's subnet broadcast is delivered.
fn test_accepts_broadcast() {
    let (fd, port) = open_udp();
    set_receive_timeout(fd, ICMP_TIMEOUT);

    send(
        fd,
        b"broadcast",
        SockAddr::inet(Ipv4Addr([127, 255, 255, 255]), port),
    )
    .expect("send");

    let mut buf = [0u8; 16];
    let reply = receive(fd, &mut buf).expect("a subnet broadcast must be delivered");
    assert_eq!(reply.data, b"broadcast", "payload");

    close(fd).expect("close");
}

/// Datagrams sent back to back on lo arrive in the order they were sent.
///
/// This can only fail if packets are reordered, but timing decides whether the old incoming
/// queue reordered them on a given run, so a pass isn't proof of FIFO order.
fn test_loopback_order() {
    // fits both the incoming queue and the socket's receive queue, so nothing is dropped
    const COUNT: u8 = 16;

    let (fd, port) = open_udp();
    set_receive_timeout(fd, ICMP_TIMEOUT);
    let dest = SockAddr::inet(Ipv4Addr::LOOPBACK, port);

    for i in 0..COUNT {
        send(fd, &[i], dest).expect("send");
    }

    let mut buf = [0u8; 8];
    for i in 0..COUNT {
        let reply = receive(fd, &mut buf).expect("receive");
        assert_eq!(reply.data, &[i], "datagram {i} out of order");
    }

    close(fd).expect("close");
}

fn lo_id() -> InterfaceId {
    loopback().expect("loopback interface").unwrap().id
}

/// A non-loopback interface, if the kernel has one (e1000 under QEMU). The tests never configure
/// it, so it has no address and no routes.
fn other_id() -> Option<InterfaceId> {
    interface_iter()
        .flatten()
        .find(|i| !i.is_loopback())
        .map(|i| i.id)
}

/// Opens a UDP socket bound to `ip` on `interface`, on an ephemeral port.
fn open_udp_on(ip: Ipv4Addr, interface: InterfaceId) -> (Fd, u16) {
    let fd = socket(SocketType::Udp, SockAddr::inet_on(ip, 0, interface)).expect("socket open");
    let port = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;
    (fd, port)
}

/// A socket bound to lo receives datagrams that arrive on lo, with or without an address. The
/// source it reports has no interface, as for unbound sockets.
fn test_bind_interface_receives() {
    for ip in [Ipv4Addr::UNSPECIFIED, Ipv4Addr::LOOPBACK] {
        let (fd, port) = open_udp_on(ip, lo_id());
        set_receive_timeout(fd, ICMP_TIMEOUT);
        let (sender, sender_port) = open_udp();

        send(sender, b"bound", SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("send");

        let mut buf = [0u8; 16];
        let reply = receive(fd, &mut buf).expect("socket bound to lo must receive");
        assert_eq!(reply.data, b"bound", "payload ({ip})");
        assert_eq!(
            reply.src,
            SockAddr::inet(Ipv4Addr::LOOPBACK, sender_port),
            "source must be the sender, with no interface ({ip})"
        );

        close(fd).expect("close");
        close(sender).expect("close");
    }
}

/// A socket bound to another interface does not receive what arrives on lo, so the port counts
/// as unbound there and lo answers with Port Unreachable.
fn test_bind_interface_filters() {
    let Some(other) = other_id() else { return };

    let capture = open_lo_capture();
    let (fd, port) = open_udp_on(Ipv4Addr::UNSPECIFIED, other);
    set_receive_timeout(fd, ICMP_TIMEOUT);
    let (sender, _) = open_udp();

    send(sender, b"x", SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("send");

    let mut frame = [0u8; 128];
    let (saw_udp, icmp) = capture_icmp(capture, &mut frame);
    assert!(saw_udp, "datagram must reach lo");
    assert!(icmp.is_some(), "lo must answer with Port Unreachable");

    let mut buf = [0u8; 8];
    assert_eq!(
        receive(fd, &mut buf).map(|r| r.len()),
        Err(SysError::TimedOut),
        "a socket bound to another interface must not receive from lo"
    );

    close(fd).expect("close");
    close(sender).expect("close");
    close(capture).expect("close");
}

/// A port can be bound once per interface, and a socket on any interface conflicts with all of
/// them.
fn test_bind_interface_conflicts() {
    const PORT: u16 = 26210;
    let lo = lo_id();
    let on_lo = SockAddr::inet_on(Ipv4Addr::UNSPECIFIED, PORT, lo);
    let any = SockAddr::any_inet_with_port(PORT);

    let fd = socket(SocketType::Udp, on_lo).expect("socket open on lo");
    assert_eq!(
        socket(SocketType::Udp, on_lo),
        Err(SysError::AlreadyExists),
        "same port on the same interface must conflict"
    );
    assert_eq!(
        socket(SocketType::Udp, any),
        Err(SysError::AlreadyExists),
        "any interface must conflict with a bound interface"
    );

    if let Some(other) = other_id() {
        let on_other = SockAddr::inet_on(Ipv4Addr::UNSPECIFIED, PORT, other);
        let fd_other =
            socket(SocketType::Udp, on_other).expect("same port on another interface must bind");
        close(fd_other).expect("close");
    }
    close(fd).expect("close");

    let fd = socket(SocketType::Udp, any).expect("socket open on any interface");
    assert_eq!(
        socket(SocketType::Udp, on_lo),
        Err(SysError::AlreadyExists),
        "a bound interface must conflict with any interface"
    );
    close(fd).expect("close");
}

/// Binding fails for an interface that doesn't exist, and for an address the interface doesn't
/// own, even if another interface does.
fn test_bind_interface_rejects() {
    assert_eq!(
        socket(
            SocketType::Udp,
            SockAddr::inet_on(Ipv4Addr::UNSPECIFIED, 0, InterfaceId(999))
        ),
        Err(SysError::NoEntry),
        "unknown interface must return NoEntry"
    );

    if let Some(other) = other_id() {
        assert_eq!(
            socket(
                SocketType::Udp,
                SockAddr::inet_on(Ipv4Addr::LOOPBACK, 0, other)
            ),
            Err(SysError::InvalidArgument),
            "an address owned by lo must not bind on another interface"
        );
    }
}

/// Interface 0 is "any interface": it binds and receives like an unbound socket.
fn test_bind_interface_zero_is_any() {
    let (fd, port) = open_udp_on(Ipv4Addr::UNSPECIFIED, InterfaceId(0));
    set_receive_timeout(fd, ICMP_TIMEOUT);

    send(fd, b"any", SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("send");

    let mut buf = [0u8; 8];
    let reply = receive(fd, &mut buf).expect("interface 0 must receive like any interface");
    assert_eq!(reply.data, b"any", "payload");

    close(fd).expect("close");
}

/// Sends from a bound socket only use routes on its interface. Another interface has no route
/// to 127.0.0.1, so the send fails instead of falling back to lo.
///
/// The other interface is unconfigured, so this checks the route filter but not the check that
/// keeps loopback destinations off non-loopback interfaces: that needs an address and a default
/// route on it.
fn test_bound_send_uses_interface() {
    let (receiver, port) = open_udp();
    set_receive_timeout(receiver, ICMP_TIMEOUT);
    let dest = SockAddr::inet(Ipv4Addr::LOOPBACK, port);

    let (on_lo, _) = open_udp_on(Ipv4Addr::UNSPECIFIED, lo_id());
    send(on_lo, b"lo", dest).expect("send from a socket bound to lo");
    let mut buf = [0u8; 8];
    let reply = receive(receiver, &mut buf).expect("receive");
    assert_eq!(reply.data, b"lo", "payload");
    close(on_lo).expect("close");

    if let Some(other) = other_id() {
        let (on_other, _) = open_udp_on(Ipv4Addr::UNSPECIFIED, other);
        assert_eq!(
            send(on_other, b"x", dest),
            Err(SysError::NetworkUnreachable),
            "a socket bound to another interface must not send through lo"
        );
        close(on_other).expect("close");
    }

    close(receiver).expect("close");
}

/// Addresses with non-zero padding are rejected, so the padding can carry new fields later
/// without old programs passing garbage in them. The same addresses with zero padding open.
fn test_rejects_nonzero_padding() {
    let cases = [
        (SocketType::Udp, SockAddr::any_inet()),
        (
            SocketType::Packet,
            SockAddr::link(Some(lo_id()), Some(ETHERTYPE_IPV4)),
        ),
    ];

    for (typ, addr) in cases {
        let mut raw_addr = RawSockAddr::from(addr);

        let fd = raw_result(raw::socket(typ as usize, &raw_addr)).expect("zero padding must open");
        close(Fd(fd)).expect("close");

        // the last byte is padding in both inet and link addresses
        raw_addr.data[13] = 1;
        assert_eq!(
            raw_result(raw::socket(typ as usize, &raw_addr)),
            Err(SysError::InvalidArgument),
            "non-zero padding must be rejected ({typ:?})"
        );
    }
}

/// Offset of the UDP checksum in a frame.
const UDP_SUM: usize = L4_START + 6;

/// RFC 768 checksum of a UDP `segment` (header and payload) from `src` to `dest`, taking its
/// checksum field as it is. A valid segment gives 0; with the field zeroed, it gives the checksum.
fn udp_checksum(src: Ipv4Addr, dest: Ipv4Addr, segment: &[u8]) -> u16 {
    let mut data = Vec::with_capacity(12 + segment.len());
    data.extend_from_slice(&src.0);
    data.extend_from_slice(&dest.0);
    data.extend_from_slice(&[0, IPPROTO_UDP]);
    data.extend_from_slice(&(segment.len() as u16).to_be_bytes());
    data.extend_from_slice(segment);
    checksum(&data)
}

/// A frame from 127.0.0.1 to 127.0.0.1 carrying `payload`, with a valid UDP checksum.
fn checksummed_udp_frame(dest_port: u16, payload: &[u8]) -> Vec<u8> {
    let lo = Ipv4Addr::LOOPBACK;
    let mut frame = raw_udp_frame(lo, lo, dest_port).to_vec();
    frame.extend_from_slice(payload);

    let ip_len = (frame.len() - IP_START) as u16;
    frame[IP_START + 2..IP_START + 4].copy_from_slice(&ip_len.to_be_bytes());
    frame[IP_START + 10..IP_START + 12].copy_from_slice(&[0, 0]);
    let ip_sum = checksum(&frame[IP_START..L4_START]);
    frame[IP_START + 10..IP_START + 12].copy_from_slice(&ip_sum.to_be_bytes());

    let udp_len = (frame.len() - L4_START) as u16;
    frame[L4_START + 4..L4_START + 6].copy_from_slice(&udp_len.to_be_bytes());
    let sum = udp_checksum(lo, lo, &frame[L4_START..]);
    frame[UDP_SUM..UDP_SUM + 2].copy_from_slice(&sum.to_be_bytes());

    frame
}

/// A datagram with a wrong checksum is dropped. A valid checksum and no checksum (0) are both
/// delivered; the valid case also shows the test computes checksums the way the kernel does.
fn test_checksum_verified() {
    const PAYLOAD: &[u8] = b"checksum";

    let capture = open_lo_capture();
    let lo = SockAddr::link(Some(lo_id()), Some(ETHERTYPE_IPV4));
    let (fd, port) = open_udp();
    set_receive_timeout(fd, ICMP_TIMEOUT);

    let valid = checksummed_udp_frame(port, PAYLOAD);
    let sum = u16::from_be_bytes([valid[UDP_SUM], valid[UDP_SUM + 1]]);
    let mut buf = [0u8; 16];

    let mut bad = valid.clone();
    bad[UDP_SUM..UDP_SUM + 2].copy_from_slice(&(sum ^ 1).to_be_bytes());
    send(capture, &bad, lo).expect("send raw frame");
    assert_eq!(
        receive(fd, &mut buf).map(|r| r.len()),
        Err(SysError::TimedOut),
        "a wrong checksum must be dropped"
    );

    let mut none = valid.clone();
    none[UDP_SUM..UDP_SUM + 2].copy_from_slice(&[0, 0]);
    for (what, frame) in [("valid checksum", &valid), ("no checksum", &none)] {
        send(capture, frame, lo).expect("send raw frame");
        let reply = receive(fd, &mut buf).unwrap_or_else(|e| panic!("{what}: {e:?}"));
        assert_eq!(reply.data, PAYLOAD, "payload ({what})");
    }

    close(fd).expect("close");
    close(capture).expect("close");
}

/// A datagram with a wrong checksum to an unbound port gets no Port Unreachable, while the same
/// datagram with a valid checksum does.
fn test_no_port_unreachable_for_bad_checksum() {
    let capture = open_lo_capture();
    let lo = SockAddr::link(Some(lo_id()), Some(ETHERTYPE_IPV4));
    let port = unbound_port();

    let valid = checksummed_udp_frame(port, b"x");
    send(capture, &valid, lo).expect("send raw frame");
    let mut buf = [0u8; 128];
    let (_, icmp) = capture_icmp(capture, &mut buf);
    assert!(
        icmp.is_some(),
        "a valid checksum to an unbound port must be answered"
    );

    let mut bad = valid;
    bad[UDP_SUM] ^= 0xff;
    send(capture, &bad, lo).expect("send raw frame");
    assert_no_icmp(capture, "a wrong checksum must not be answered");

    close(capture).expect("close");
}

/// A checksum that computes to 0 is sent as 0xffff, since 0 means "no checksum" (RFC 768), and
/// the receiver accepts it.
fn test_zero_checksum_sent_as_ones() {
    let capture = open_lo_capture();
    let (fd, port) = open_udp();
    set_receive_timeout(fd, ICMP_TIMEOUT);

    // the segment the kernel will build, with a zero payload word. Adding a word equal to its
    // checksum makes the sum all ones, so the computed checksum becomes 0.
    let mut segment = [0u8; 10];
    segment[0..2].copy_from_slice(&port.to_be_bytes());
    segment[2..4].copy_from_slice(&port.to_be_bytes());
    segment[4..6].copy_from_slice(&10u16.to_be_bytes());
    let lo = Ipv4Addr::LOOPBACK;
    let payload = udp_checksum(lo, lo, &segment).to_be_bytes();

    send(fd, &payload, SockAddr::inet(lo, port)).expect("send");

    let mut frame = [0u8; 128];
    loop {
        let _ = receive(capture, &mut frame).expect("datagram must reach lo");
        let dest_port = u16::from_be_bytes([frame[L4_START + 2], frame[L4_START + 3]]);
        if frame[IP_PROTO] == IPPROTO_UDP && dest_port == port {
            break;
        }
    }
    assert_eq!(
        &frame[UDP_SUM..UDP_SUM + 2],
        &[0xff, 0xff],
        "a zero checksum must be sent as all ones"
    );

    let mut buf = [0u8; 8];
    let reply = receive(fd, &mut buf).expect("an all ones checksum must be accepted");
    assert_eq!(reply.data, &payload, "payload");

    close(fd).expect("close");
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
    test_send_rejects_huge_length();
    test_port_unreachable();
    test_no_port_unreachable_when_bound();
    test_no_port_unreachable_for_broadcast();
    test_no_port_unreachable_for_bad_source();
    test_drops_other_hosts();
    test_accepts_broadcast();
    test_loopback_order();
    test_bind_interface_receives();
    test_bind_interface_filters();
    test_bind_interface_conflicts();
    test_bind_interface_rejects();
    test_bind_interface_zero_is_any();
    test_bound_send_uses_interface();
    test_rejects_nonzero_padding();
    test_checksum_verified();
    test_no_port_unreachable_for_bad_checksum();
    test_zero_checksum_sent_as_ones();
}
