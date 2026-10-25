#![no_std]
#![no_main]

use user::*;

const ETH_HEADER_SIZE: usize = 14;
const ETH_MAX_FRAME: usize = 1514;
const ETHERTYPE_IPV4: u16 = 0x0800;

/// IEEE 802 local experimental EtherTypes; the kernel never handles these itself.
const ETHERTYPE_TEST: u16 = 0x88b5;
const ETHERTYPE_TEST2: u16 = 0x88b6;

const TEST_SRC_MAC: MacAddr = MacAddr([0x02, 0x00, 0x00, 0x00, 0x00, 0x01]);
const BROADCAST_MAC: MacAddr = MacAddr([0xff; 6]);

/// Ticks to wait before deciding a frame is not coming.
const TIMEOUT: usize = 3;

const PAYLOAD: &[u8] = b"octopos packet test";

fn loopback_id() -> InterfaceId {
    loopback().expect("loopback interface").unwrap().id
}

fn open_packet(interface: Option<InterfaceId>, ether_type: Option<u16>) -> Result<Fd, SysError> {
    let fd = socket(SocketType::Packet, SockAddr::link(interface, ether_type))?;
    set_receive_timeout(fd, 3 * TIMEOUT);
    Ok(fd)
}

fn set_receive_timeout(fd: Fd, ticks: usize) {
    ioctl(fd, Ioctl::SOCKET_SET_RECV_TIMEOUT, ticks).expect("ioctl set receive timeout");
}

/// Writes an Ethernet frame into `buf` and returns its length.
fn frame(buf: &mut [u8], src: MacAddr, ether_type: u16, payload: &[u8]) -> usize {
    let len = ETH_HEADER_SIZE + payload.len();

    buf[0..6].copy_from_slice(&BROADCAST_MAC.0);
    buf[6..12].copy_from_slice(&src.0);
    buf[12..14].copy_from_slice(&ether_type.to_be_bytes());
    buf[ETH_HEADER_SIZE..len].copy_from_slice(payload);

    len
}

/// Asserts that no frame arrives on `fd` within `TIMEOUT`.
fn assert_nothing_received(fd: Fd, msg: &str) {
    set_receive_timeout(fd, TIMEOUT);
    let mut buf = [0u8; 64];
    assert_eq!(
        receive(fd, &mut buf).map(|(n, _)| n),
        Err(SysError::TimedOut),
        "{msg}"
    );
}

/// Opening and closing works, and the same filter can be opened again.
fn test_open_close() {
    let lo = loopback_id();

    let fd = open_packet(Some(lo), Some(ETHERTYPE_TEST)).expect("socket open");
    close(fd).expect("close");

    let fd = open_packet(Some(lo), Some(ETHERTYPE_TEST)).expect("socket reopen after close");
    close(fd).expect("close");
}

/// A frame sent on lo comes back unchanged, with the link-layer source filled in.
fn test_round_trip() {
    let lo = loopback_id();
    let fd = open_packet(Some(lo), Some(ETHERTYPE_TEST)).expect("socket open");

    let mut out = [0u8; 64];
    let len = frame(&mut out, TEST_SRC_MAC, ETHERTYPE_TEST, PAYLOAD);

    // interface None falls back to the bound interface
    let n = send(fd, &out[..len], SockAddr::any_link()).expect("send");
    assert_eq!(n, len, "send must return frame length");

    let mut buf = [0u8; 64];
    let (n, src) = receive(fd, &mut buf).expect("receive");

    assert_eq!(
        &buf[..n],
        &out[..len],
        "received frame must equal sent frame"
    );
    assert_eq!(
        src,
        SockAddr::Link {
            interface: Some(lo),
            ether_type: Some(ETHERTYPE_TEST),
            mac: TEST_SRC_MAC,
        },
        "source must carry the receiving interface, EtherType and source MAC"
    );

    close(fd).expect("close");
}

/// Sockets only receive frames of the EtherType they are bound to.
fn test_ethertype_filter() {
    let lo = loopback_id();
    let fd_test = open_packet(Some(lo), Some(ETHERTYPE_TEST)).expect("socket open");
    let fd_test2 = open_packet(Some(lo), Some(ETHERTYPE_TEST2)).expect("socket open");

    let mut out = [0u8; 64];
    let len = frame(&mut out, TEST_SRC_MAC, ETHERTYPE_TEST2, PAYLOAD);
    send(fd_test2, &out[..len], SockAddr::any_link()).expect("send");

    let mut buf = [0u8; 64];
    let (n, _) = receive(fd_test2, &mut buf).expect("matching socket must receive");
    assert_eq!(&buf[..n], &out[..len], "frame mismatch");

    assert_nothing_received(
        fd_test,
        "socket bound to another EtherType must not receive",
    );

    close(fd_test).expect("close");
    close(fd_test2).expect("close");
}

/// Every matching socket gets its own copy: two on the same filter, plus one on all EtherTypes.
fn test_fan_out_to_packet_sockets() {
    let lo = loopback_id();
    let fd1 = open_packet(Some(lo), Some(ETHERTYPE_TEST)).expect("socket open");
    let fd2 = open_packet(Some(lo), Some(ETHERTYPE_TEST)).expect("socket open");
    let fd_all = open_packet(Some(lo), None).expect("socket open");

    let mut out = [0u8; 64];
    let len = frame(&mut out, TEST_SRC_MAC, ETHERTYPE_TEST, PAYLOAD);
    send(fd1, &out[..len], SockAddr::any_link()).expect("send");

    for fd in [fd1, fd2, fd_all] {
        let mut buf = [0u8; 64];
        let (n, src) = receive(fd, &mut buf).expect("every matching socket must receive");
        assert_eq!(&buf[..n], &out[..len], "frame mismatch");
        assert_eq!(
            src,
            SockAddr::Link {
                interface: Some(lo),
                ether_type: Some(ETHERTYPE_TEST),
                mac: TEST_SRC_MAC,
            },
            "an all-EtherType socket must report the frame's real EtherType"
        );
    }

    close(fd1).expect("close");
    close(fd2).expect("close");
    close(fd_all).expect("close");
}

/// A packet socket sees IPv4 traffic without taking it away from the kernel's UDP socket.
fn test_fan_out_with_kernel() {
    const IPV4_HEADER_SIZE: usize = 20;
    const UDP_HEADER_SIZE: usize = 8;

    let lo = loopback_id();
    let fd_packet = open_packet(Some(lo), Some(ETHERTYPE_IPV4)).expect("packet socket open");
    let fd_udp = socket(SocketType::Udp, SockAddr::any_inet()).expect("udp socket open");
    let port = ioctl(fd_udp, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get port") as u16;

    send(fd_udp, PAYLOAD, SockAddr::inet(Ipv4Addr::LOOPBACK, port)).expect("udp send");

    let mut buf = [0u8; 128];
    let (n, _) = receive(fd_udp, &mut buf).expect("udp socket must still receive");
    assert_eq!(&buf[..n], PAYLOAD, "udp payload mismatch");

    let (n, src) = receive(fd_packet, &mut buf).expect("packet socket must receive a copy");
    assert_eq!(
        src,
        SockAddr::Link {
            interface: Some(lo),
            ether_type: Some(ETHERTYPE_IPV4),
            mac: MacAddr(buf[6..12].try_into().unwrap()),
        },
        "source mismatch"
    );
    assert_eq!(
        u16::from_be_bytes([buf[12], buf[13]]),
        ETHERTYPE_IPV4,
        "frame must keep its Ethernet header"
    );
    assert_eq!(buf[ETH_HEADER_SIZE], 0x45, "expected IPv4 without options");

    let start = ETH_HEADER_SIZE + IPV4_HEADER_SIZE + UDP_HEADER_SIZE;
    assert!(n >= start + PAYLOAD.len(), "frame too short");
    assert_eq!(
        &buf[start..start + PAYLOAD.len()],
        PAYLOAD,
        "frame must carry the UDP payload"
    );

    close(fd_packet).expect("close");
    close(fd_udp).expect("close");
}

/// A socket bound to any interface needs one at send time, and reports where frames arrived.
fn test_unbound_interface() {
    let lo = loopback_id();
    let fd = open_packet(None, Some(ETHERTYPE_TEST)).expect("socket open");

    let mut out = [0u8; 64];
    let len = frame(&mut out, TEST_SRC_MAC, ETHERTYPE_TEST, PAYLOAD);

    assert_eq!(
        send(fd, &out[..len], SockAddr::any_link()),
        Err(SysError::NotPermitted),
        "send with no interface anywhere must fail"
    );

    send(fd, &out[..len], SockAddr::link(Some(lo), None)).expect("send on explicit interface");

    let mut buf = [0u8; 64];
    let (n, src) = receive(fd, &mut buf).expect("receive");
    assert_eq!(&buf[..n], &out[..len], "frame mismatch");
    assert_eq!(
        src,
        SockAddr::Link {
            interface: Some(lo),
            ether_type: Some(ETHERTYPE_TEST),
            mac: TEST_SRC_MAC,
        },
        "an any-interface socket must report the receiving interface"
    );

    close(fd).expect("close");
}

/// Bad bind addresses and bad send destinations are rejected.
fn test_reject_invalid_addresses() {
    assert_eq!(
        open_packet(Some(InterfaceId(0xfffe)), None),
        Err(SysError::NoEntry),
        "bind to a missing interface must fail"
    );
    assert_eq!(
        socket(SocketType::Packet, SockAddr::any_inet()),
        Err(SysError::InvalidArgument),
        "packet socket must reject an inet bind address"
    );
    assert_eq!(
        socket(SocketType::Udp, SockAddr::any_link()),
        Err(SysError::InvalidArgument),
        "udp socket must reject a link bind address"
    );

    let lo = loopback_id();
    let fd = open_packet(Some(lo), Some(ETHERTYPE_TEST)).expect("socket open");

    let mut out = [0u8; 64];
    let len = frame(&mut out, TEST_SRC_MAC, ETHERTYPE_TEST, PAYLOAD);
    assert_eq!(
        send(fd, &out[..len], SockAddr::inet(Ipv4Addr::LOOPBACK, 0)),
        Err(SysError::InvalidArgument),
        "send with an inet destination must fail"
    );
    assert_eq!(
        send(
            fd,
            &out[..len],
            SockAddr::link(Some(InterfaceId(0xfffe)), None)
        ),
        Err(SysError::NoEntry),
        "send on a missing interface must fail"
    );

    assert_eq!(
        ioctl(fd, Ioctl::SOCKET_GET_PORT, 0),
        Err(SysError::InvalidArgument),
        "packet sockets have no port"
    );

    close(fd).expect("close");
}

/// Frames must hold an Ethernet header and fit in a standard frame; both bounds are inclusive.
fn test_frame_size_limits() {
    let lo = loopback_id();
    let fd = open_packet(Some(lo), Some(ETHERTYPE_TEST)).expect("socket open");
    let dest = SockAddr::link(None, None);

    let mut out = [0u8; ETH_MAX_FRAME + 1];
    frame(&mut out, TEST_SRC_MAC, ETHERTYPE_TEST, &[]);

    assert_eq!(
        send(fd, &out[..ETH_HEADER_SIZE - 1], dest),
        Err(SysError::InvalidArgument),
        "frame shorter than the Ethernet header must be rejected"
    );
    assert_eq!(
        send(fd, &out[..ETH_MAX_FRAME + 1], dest),
        Err(SysError::MessageTooLarge),
        "frame longer than 1514 bytes must be rejected"
    );

    let mut buf = [0u8; ETH_MAX_FRAME + 1];

    send(fd, &out[..ETH_HEADER_SIZE], dest).expect("header-only frame must be accepted");
    let (n, _) = receive(fd, &mut buf).expect("receive header-only frame");
    assert_eq!(n, ETH_HEADER_SIZE, "header-only frame length mismatch");

    send(fd, &out[..ETH_MAX_FRAME], dest).expect("1514-byte frame must be accepted");
    let (n, _) = receive(fd, &mut buf).expect("receive full-size frame");
    assert_eq!(n, ETH_MAX_FRAME, "full-size frame length mismatch");

    close(fd).expect("close");
}

/// The receive timeout applies to packet sockets.
fn test_receive_timeout() {
    let lo = loopback_id();
    let fd = open_packet(Some(lo), Some(ETHERTYPE_TEST)).expect("socket open");

    let start = uptime();
    assert_nothing_received(fd, "receive with nothing sent must time out");
    assert!(
        uptime() - start >= TIMEOUT,
        "receive must not time out before the deadline"
    );

    close(fd).expect("close");
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_open_close();
    test_round_trip();
    test_ethertype_filter();
    test_fan_out_to_packet_sockets();
    test_fan_out_with_kernel();
    test_unbound_interface();
    test_reject_invalid_addresses();
    test_frame_size_limits();
    test_receive_timeout();
}
