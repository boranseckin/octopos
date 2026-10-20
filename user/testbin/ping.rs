#![no_std]
#![no_main]

use user::*;

const ICMP_ECHO_REPLY: u8 = 0;
const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_HEADER_SIZE: usize = 8;

/// Odd length on purpose, to exercise the trailing-byte path of the checksum.
const PAYLOAD: &[u8] = b"octopos ping test";

/// Writes an Echo Request into `buf` and returns its length.
fn echo_request(buf: &mut [u8], ident: u16, seq: u16, sum: u16, payload: &[u8]) -> usize {
    let len = ICMP_HEADER_SIZE + payload.len();

    buf[0] = ICMP_ECHO_REQUEST;
    buf[1] = 0; // code
    buf[2..4].copy_from_slice(&sum.to_be_bytes());
    buf[4..6].copy_from_slice(&ident.to_be_bytes());
    buf[6..8].copy_from_slice(&seq.to_be_bytes());
    buf[ICMP_HEADER_SIZE..len].copy_from_slice(payload);

    len
}

/// RFC 1071 internet checksum. A message with a valid checksum sums to zero.
fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    let mut chunks = data.chunks_exact(2);
    for chunk in &mut chunks {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    if let [byte] = chunks.remainder() {
        sum += (*byte as u32) << 8;
    }

    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }

    !(sum as u16)
}

fn open_ping(ident: u16) -> Result<(Fd, u16), SysError> {
    let fd = socket(SocketType::Ping, SockAddr::any_inet_with_port(ident))?;
    let ident = ioctl(fd, Ioctl::SOCKET_GET_PORT, 0).expect("ioctl get ident") as u16;
    Ok((fd, ident))
}

/// Sends an Echo Request to loopback and checks every field of the reply.
fn ping_loopback(fd: Fd, ident: u16, seq: u16, request: &[u8]) {
    let n = send(fd, request, SockAddr::inet(Ipv4Addr::LOOPBACK, 0)).expect("send");
    assert_eq!(n, request.len(), "send must return message length");

    let mut buf = [0u8; 64];
    let (len, src) = receive(fd, &mut buf).expect("receive");
    let reply = &buf[..len];

    assert_eq!(
        src,
        SockAddr::inet(Ipv4Addr::LOOPBACK, ident),
        "reply source must be loopback, with the socket's ident as port"
    );
    assert_eq!(
        len,
        ICMP_HEADER_SIZE + PAYLOAD.len(),
        "reply must be the full ICMP message"
    );
    assert_eq!(reply[0], ICMP_ECHO_REPLY, "reply type must be Echo Reply");
    assert_eq!(reply[1], 0, "reply code must be zero");
    assert_eq!(checksum(reply), 0, "reply checksum must be valid");
    assert_eq!(
        u16::from_be_bytes([reply[4], reply[5]]),
        ident,
        "reply ident must be the socket's ident"
    );
    assert_eq!(
        u16::from_be_bytes([reply[6], reply[7]]),
        seq,
        "seq mismatch"
    );
    assert_eq!(&reply[ICMP_HEADER_SIZE..], PAYLOAD, "payload mismatch");
}

/// A well-formed Echo Request to loopback gets a matching Echo Reply.
fn test_echo_round_trip() {
    let (fd, ident) = open_ping(0).expect("socket open");

    let mut request = [0u8; 64];
    let len = echo_request(&mut request, 0, 7, 0, PAYLOAD);
    ping_loopback(fd, ident, 7, &request[..len]);

    close(fd).expect("close");
}

/// The kernel must overwrite the user's ident and recompute the checksum, so garbage in both
/// fields still produces a valid reply addressed to this socket.
fn test_ident_and_checksum_overwritten() {
    let (fd, ident) = open_ping(0).expect("socket open");

    let mut request = [0u8; 64];
    let len = echo_request(&mut request, ident ^ 0xbeef, 3, 0xdead, PAYLOAD);
    ping_loopback(fd, ident, 3, &request[..len]);

    close(fd).expect("close");
}

/// Replies are delivered by ident, so each socket receives only its own.
fn test_demux_by_ident() {
    let (fd1, ident1) = open_ping(0).expect("socket open");
    let (fd2, ident2) = open_ping(0).expect("socket open");
    assert_ne!(ident1, ident2, "ephemeral idents must be unique");

    let mut request = [0u8; 64];
    let len = echo_request(&mut request, 0, 1, 0, PAYLOAD);
    let dest = SockAddr::inet(Ipv4Addr::LOOPBACK, 0);

    // both replies are queued before either socket receives
    send(fd1, &request[..len], dest).expect("send");
    send(fd2, &request[..len], dest).expect("send");

    let mut buf = [0u8; 64];
    let (len1, _) = receive(fd1, &mut buf).expect("receive");
    assert!(len1 >= ICMP_HEADER_SIZE, "reply too short");
    assert_eq!(
        u16::from_be_bytes([buf[4], buf[5]]),
        ident1,
        "fd1 got another socket's reply"
    );

    let (len2, _) = receive(fd2, &mut buf).expect("receive");
    assert!(len2 >= ICMP_HEADER_SIZE, "reply too short");
    assert_eq!(
        u16::from_be_bytes([buf[4], buf[5]]),
        ident2,
        "fd2 got another socket's reply"
    );

    close(fd1).expect("close");
    close(fd2).expect("close");
}

/// Ephemeral idents are never 0, which is reserved for "pick one".
fn test_ephemeral_ident() {
    let (fd, ident) = open_ping(0).expect("socket open");
    assert_ne!(ident, 0, "ephemeral ident must not be 0");
    close(fd).expect("close");
}

/// Binding an ident that is already in use fails, and it can be reused after closing.
fn test_duplicate_ident() {
    let (fd, ident) = open_ping(4242).expect("socket open");
    assert_eq!(ident, 4242, "explicit ident must be kept");

    assert_eq!(
        open_ping(4242).map(|(fd, _)| fd),
        Err(SysError::AlreadyExists),
        "duplicate ident must return AlreadyExists"
    );

    close(fd).expect("close");

    let (fd, _) = open_ping(4242).expect("socket reopen after close");
    close(fd).expect("close");
}

/// Ping sockets only send Echo Requests, and only full ICMP headers.
fn test_reject_invalid_messages() {
    let (fd, _) = open_ping(0).expect("socket open");
    let dest = SockAddr::inet(Ipv4Addr::LOOPBACK, 0);

    let mut request = [0u8; 64];
    let len = echo_request(&mut request, 0, 0, 0, PAYLOAD);

    request[0] = ICMP_ECHO_REPLY;
    assert_eq!(
        send(fd, &request[..len], dest),
        Err(SysError::InvalidArgument),
        "non Echo Request type must be rejected"
    );

    request[0] = ICMP_ECHO_REQUEST;
    request[1] = 1;
    assert_eq!(
        send(fd, &request[..len], dest),
        Err(SysError::InvalidArgument),
        "non-zero code must be rejected"
    );

    request[1] = 0;
    assert_eq!(
        send(fd, &request[..ICMP_HEADER_SIZE - 1], dest),
        Err(SysError::InvalidArgument),
        "message shorter than the ICMP header must be rejected"
    );

    close(fd).expect("close");
}

/// Ping sockets share the receive queue with UDP, so the timeout applies to them too.
fn test_receive_timeout() {
    const TIMEOUT: usize = 3;

    let (fd, _) = open_ping(0).expect("socket open");
    ioctl(fd, Ioctl::SOCKET_SET_RECV_TIMEOUT, TIMEOUT).expect("ioctl set receive timeout");

    let start = uptime();
    let mut buf = [0u8; 64];
    assert_eq!(
        receive(fd, &mut buf).map(|(n, _)| n),
        Err(SysError::TimedOut),
        "receive with no request sent must time out"
    );
    assert!(
        uptime() - start >= TIMEOUT,
        "receive must not time out before the deadline"
    );

    // a reply still arrives before the timeout
    let mut request = [0u8; 64];
    let len = echo_request(&mut request, 0, 9, 0, PAYLOAD);
    send(fd, &request[..len], SockAddr::inet(Ipv4Addr::LOOPBACK, 0)).expect("send");
    receive(fd, &mut buf).expect("reply must arrive before the timeout");

    close(fd).expect("close");
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_echo_round_trip();
    test_ident_and_checksum_overwritten();
    test_demux_by_ident();
    test_ephemeral_ident();
    test_duplicate_ident();
    test_reject_invalid_messages();
    test_receive_timeout();
}
