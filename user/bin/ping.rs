#![no_std]
#![no_main]

use user::*;

const USAGE: &str = "usage: ping <address> [count]";

const DEFAULT_COUNT: u16 = 4;

/// Delay between echo requests. One tick is 100ms on QEMU virt.
const INTERVAL_TICKS: usize = 10;

const ICMP_ECHO_REQUEST: u8 = 8;
const ICMP_ECHO_REPLY: u8 = 0;

/// ICMP header (type, code, checksum, ident, seq) followed by the send time in ticks.
const ICMP_HEADER_SIZE: usize = 8;
const MSG_SIZE: usize = ICMP_HEADER_SIZE + size_of::<u64>();

/// Builds an Echo Request. Checksum and ident are left zero; the kernel fills them in.
fn echo_request(seq: u16, sent: u64) -> [u8; MSG_SIZE] {
    let mut msg = [0u8; MSG_SIZE];

    msg[0] = ICMP_ECHO_REQUEST;
    msg[1] = 0; // code
    // msg[2..4]: checksum, msg[4..6]: ident
    msg[6..8].copy_from_slice(&seq.to_be_bytes());
    msg[8..].copy_from_slice(&sent.to_be_bytes());

    msg
}

/// Parses an Echo Reply, returning its seq and send time, or `None` if it is not a reply.
fn parse_echo_reply(msg: &[u8]) -> Option<(u16, u64)> {
    if msg.len() < MSG_SIZE || msg[0] != ICMP_ECHO_REPLY {
        return None;
    }

    let seq = u16::from_be_bytes(msg[6..8].try_into().ok()?);
    let sent = u64::from_be_bytes(msg[8..MSG_SIZE].try_into().ok()?);

    Some((seq, sent))
}

#[unsafe(no_mangle)]
fn main(args: Args) {
    let Some(dest_ip) = args.get_str(1) else {
        exit_with_msg(USAGE);
    };
    let dest_ip = dest_ip.parse::<Ipv4Addr>().unwrap_or_else(|_| {
        exit_with_msg("ping: invalid IP address");
    });
    // port is ignored by ping sockets, the kernel uses the socket's ident
    let dest = SockAddr::inet(dest_ip, 0);

    let count = match args.get_str(2) {
        Some(count) => count.parse::<u16>().unwrap_or_else(|_| {
            exit_with_msg("ping: invalid count");
        }),
        None => DEFAULT_COUNT,
    };

    let fd = socket(SocketType::Ping, SockAddr::any_inet()).expect("socket open");

    for seq in 0..count {
        let msg = echo_request(seq, uptime() as u64);
        send(fd, &msg, dest).expect("socket send");

        // skip stale replies to earlier requests until ours arrives
        let mut buf = [0u8; 64];
        loop {
            let (len, src) = receive(fd, &mut buf).expect("socket receive");
            let SockAddr::Inet { ip, .. } = src;

            match parse_echo_reply(&buf[..len]) {
                Some((reply_seq, sent)) if reply_seq == seq => {
                    let rtt = (uptime() as u64).saturating_sub(sent);
                    println!("{} bytes from {}: seq={} time={} ticks", len, ip, seq, rtt);
                    break;
                }
                _ => continue,
            }
        }

        if seq + 1 < count {
            let _ = sleep(INTERVAL_TICKS);
        }
    }

    let _ = close(fd);
}
