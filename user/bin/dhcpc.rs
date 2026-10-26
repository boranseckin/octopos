#![no_std]
#![no_main]

use user::*;

const USAGE: &str = "usage: dhcpc <dev>";

const CLIENT_PORT: u16 = 68;
const SERVER_PORT: u16 = 67;

const MAX_FRAME: usize = 1514;

const DHCP_ROUTE_METRIC: u32 = 100;

const INITIAL_INTERVAL: usize = 4 * TICKS_PER_SEC;
const MAX_INTERVAL: usize = 64 * TICKS_PER_SEC;
const JITTER: isize = TICKS_PER_SEC as isize; // +/- 1s
const MAX_ATTEMPTS: u32 = 4;

/// Vendor Information "Magic Cookie"
/// https://www.rfc-editor.org/rfc/rfc1497
const MAGIC_COOKIE: [u8; 4] = [0x63, 0x82, 0x53, 0x63];

const DISCOVER: u8 = 1;
const OFFER: u8 = 2;
const REQUEST: u8 = 3;
const ACK: u8 = 5;
const NAK: u8 = 6;

const OPT_SUBNET_MASK: u8 = 1;
const OPT_ROUTER: u8 = 3;
const OPT_REQUESTED_IP: u8 = 50;
const OPT_LEASE_TIME: u8 = 51;
const OPT_MESSAGE_TYPE: u8 = 53;
const OPT_SERVER_ID: u8 = 54;
const OPT_PARAM_LIST: u8 = 55;
const OPT_RENEWAL_TIME: u8 = 58;
const OPT_REBINDING_TIME: u8 = 59;
const OPT_PAD: u8 = 0;
const OPT_END: u8 = 255;

/// DHCP's BOOTP header structure as defined in RFC 2131
/// https://www.rfc-editor.org/rfc/rfc2131
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct BootpHeader {
    /// Message op code (1 = BOOTREQUEST, 2 = BOOTREPLY)
    op: u8,
    /// Hardware address type (1 = Ethernet)
    htype: u8,
    /// Hardware address length (6 for Ethernet)
    hlen: u8,
    /// Client sets to zero, optionally used by relay agents when booting via a relay agent
    hops: u8,
    /// Transaction ID, a random number chosen by the client
    xid: Be<u32>,
    /// Seconds elapsed since client began address acquisition or renewal process
    secs: Be<u16>,
    /// Flags
    flags: Be<u16>,
    /// Client IP address (0.0.0.0 on first request, client fills in if renewing)
    ciaddr: Ipv4Addr,
    /// 'your' IP address (server fills this with the offered address)
    yiaddr: Ipv4Addr,
    /// Next server IP address
    siaddr: Ipv4Addr,
    /// Relay agent IP address
    giaddr: Ipv4Addr,
    /// Client hardware address
    chaddr: [u8; 16],
    /// Optional server host name, null-terminated string
    sname: [u8; 64],
    /// Optional boot filename, null-terminated string
    file: [u8; 128],
}

impl BootpHeader {
    /// Creates a new BOOTREQUEST header with default values, to be filled in by the caller.
    fn new_request() -> Self {
        Self {
            op: 1, // op=1 for BOOTREQUEST
            htype: 1,
            hlen: 6,
            hops: 0,
            xid: Be::new(0), // to be filled with a random transaction ID
            secs: Be::new(0),
            flags: Be::new(0),
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr: Ipv4Addr::UNSPECIFIED,
            siaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: [0; 16], // to be filled with client's MAC address
            sname: [0; 64],
            file: [0; 128],
        }
    }
}

unsafe impl NetworkHeader for BootpHeader {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Selecting,
    Requesting,
    Bound,
    Renewing,
    Rebinding,
}

#[derive(Debug, Clone, Copy)]
struct Offer {
    addr: Ipv4Addr,
    prefix_len: u8,
    server_id: Ipv4Addr,
    router: Option<Ipv4Addr>,
    lease_secs: u32,
    t1_secs: u32,
    t2_secs: u32,
}

#[derive(Debug)]
struct Client {
    fd: Fd,
    interface: Interface,

    /// Current state.
    state: State,

    /// Current transaction.
    /// Set by discover() or renew().
    xid: u32,

    /// Retransmissions of the current message.
    /// Set by request().
    attempts: u32,

    /// When on_timeout will trigger (ticks).
    /// Set by send().
    deadline: usize,

    /// When the last Request went out (ticks).
    /// Lease's time will start here.
    /// Set by send().
    sent_at: usize,

    /// When the client enters the Renewing state (ticks).
    /// Set by bind().
    t1_at: usize,

    /// When the client enters the Rebinding state (ticks).
    /// Set by bind().
    t2_at: usize,

    /// When the current lease expires (ticks).
    /// Set by bind().
    expires_at: usize,

    /// The offer being requested for Requesting state. Set by request().
    /// The ack in use for Bound/Renewing/Rebinding states. Set by bind().
    /// Unset by expire().
    offer: Option<Offer>,

    /// Source MAC of the last Ack.
    /// Next hop for a unicast renew.
    /// Set by bind().
    server_mac: MacAddr,
}

impl Client {
    fn discover(&mut self, now: usize) {
        println!("dhcpc: discovering");

        self.state = State::Selecting;
        self.xid = new_xid();
        self.attempts = 0;

        self.send(now);
    }

    fn request(&mut self, offer: Offer, now: usize) {
        println!("dhcpc: requesting {} from {}", offer.addr, offer.server_id);

        self.state = State::Requesting;
        self.offer = Some(offer);
        self.attempts = 0;

        self.send(now);
    }

    fn bind(&mut self, ack: Offer, mac: MacAddr) {
        println!(
            "dhcpc: bound to {}/{} via {} (valid for {}s, renew in {}s)",
            ack.addr,
            ack.prefix_len,
            ack.router
                .map_or_else(|| "-".to_string(), |r| r.to_string()),
            ack.lease_secs,
            ack.t1_secs,
        );

        self.state = State::Bound;
        self.offer = Some(ack);
        self.server_mac = mac;
        self.t1_at = self
            .sent_at
            .saturating_add((ack.t1_secs as usize).saturating_mul(TICKS_PER_SEC));
        self.t2_at = self
            .sent_at
            .saturating_add((ack.t2_secs as usize).saturating_mul(TICKS_PER_SEC));
        self.expires_at = self
            .sent_at
            .saturating_add((ack.lease_secs as usize).saturating_mul(TICKS_PER_SEC));
        self.deadline = self.t1_at;

        apply(&self.interface, &ack);
    }

    fn renew(&mut self, now: usize) {
        let offer = self.offer.expect("offer in renew");
        println!("dhcpc: renewing {} from {}", offer.addr, offer.server_id);

        self.state = State::Renewing;
        self.xid = new_xid();

        self.send(now);
    }

    fn rebind(&mut self, now: usize) {
        let offer = self.offer.expect("offer in rebind");
        println!("dhcpc: rebinding {}", offer.addr);

        self.state = State::Rebinding;

        self.send(now);
    }

    fn expire(&mut self, now: usize) {
        println!("dhcpc: expired");

        unapply(&self.interface, &self.offer.take());

        self.discover(now);
    }

    fn on_reply(&mut self, frame: &[u8], mac: MacAddr) {
        let Some((header, data)) = parse_reply(frame, &self.interface) else {
            return;
        };

        if header.xid.get() != self.xid {
            return;
        }

        let Some(msg_type) = find_option_array(data, OPT_MESSAGE_TYPE).map(u8::from_be_bytes)
        else {
            return;
        };

        match (self.state, msg_type) {
            (State::Selecting, OFFER) => {
                let Some(offer) = parse_offer(header, data) else {
                    return;
                };

                self.request(offer, uptime());
            }

            (State::Requesting | State::Renewing | State::Rebinding, ACK) => {
                let offer = self.offer.expect("offer in ack");
                let Some(ack) = parse_offer(header, data) else {
                    return;
                };

                if offer.addr != ack.addr {
                    return;
                }

                // rebind ack might come from a different server.
                if self.state != State::Rebinding && offer.server_id != ack.server_id {
                    return;
                }

                self.bind(ack, mac);
            }

            (State::Requesting | State::Renewing | State::Rebinding, NAK) => {
                let server_id = find_option_array(data, OPT_SERVER_ID)
                    .map(Ipv4Addr)
                    .expect("server id in nak");
                println!("dhcpc: nak from {}", server_id);

                if self.state != State::Requesting {
                    unapply(&self.interface, &self.offer);
                }

                self.discover(uptime());
            }

            _ => (),
        }
    }

    fn on_timeout(&mut self, now: usize) {
        match self.state {
            State::Requesting if self.attempts + 1 >= MAX_ATTEMPTS => self.discover(now),
            State::Bound => self.renew(now),
            State::Renewing if now >= self.t2_at => self.rebind(now),
            State::Rebinding if now >= self.expires_at => self.expire(now),

            State::Selecting | State::Requesting | State::Renewing | State::Rebinding => {
                self.attempts += 1;
                println!("dhcpc: retrying (on attempt {})", self.attempts + 1); // starts at 0
                self.send(now);
            }
        }
    }

    fn send(&mut self, now: usize) {
        let mut buf = [0u8; MAX_FRAME];

        let msg_type = match self.state {
            State::Selecting => DISCOVER,
            _ => REQUEST,
        };

        let (dst_mac, dst_ip) = match self.state {
            State::Renewing => (
                self.server_mac,
                self.offer.expect("offer in renewing").server_id,
            ),
            _ => (MacAddr::BROADCAST, Ipv4Addr::BROADCAST),
        };

        let ciaddr = match self.state {
            State::Renewing | State::Rebinding => {
                self.offer.expect("offer to in renewing/rebinding").addr
            }
            _ => Ipv4Addr::UNSPECIFIED,
        };

        let (requested_ip, server_id) = match self.state {
            State::Requesting => {
                let Offer {
                    addr, server_id, ..
                } = self.offer.expect("offer in requesting");
                (Some(addr), Some(server_id))
            }
            _ => (None, None),
        };

        let len = build_frame(
            &mut buf,
            &self.interface,
            msg_type,
            self.xid,
            dst_mac,
            dst_ip,
            ciaddr,
            requested_ip,
            server_id,
        );

        transmit(self.fd, &self.interface, &buf[..len]);

        self.sent_at = now;
        self.deadline = match self.state {
            State::Selecting | State::Requesting => now + backoff(self.attempts),
            State::Renewing => halfway(now, self.t2_at),
            State::Rebinding => halfway(now, self.expires_at),
            State::Bound => unreachable!("should not send while in Bound"),
        };
    }
}

fn jittered(interval: usize) -> usize {
    let mut rand = [0u8; 1];
    random(&mut rand).unwrap_or_else(|e| exit_with_msg(e));
    let jitter = rand[0] as isize % (2 * JITTER + 1) - JITTER;
    interval.saturating_add_signed(jitter)
}

/// Starting from `INITIAL_INTERVAL`, doubles the amount for each attempt, capped at `MAX_INTERVAL`,
/// with +/- `JITTER` applied.
fn backoff(attempts: u32) -> usize {
    jittered(
        INITIAL_INTERVAL
            .checked_shl(attempts.min(MAX_ATTEMPTS))
            .unwrap_or(MAX_INTERVAL),
    )
}

/// RFC 2131 4.4.5: the client SHOULD wait one-half of the remaining time until [deadline], down to
/// a minimum of 60 seconds,
fn halfway(now: usize, until: usize) -> usize {
    // let remaining = until.saturating_sub(now);
    // let half_remaining = remaining / 2;
    // let min_60_sec = half_remaining.max(60 * TICKS_PER_SEC);
    // let cannot_pass_until = min_60_sec.min(until);

    now.saturating_add((until.saturating_sub(now) / 2).max(60 * TICKS_PER_SEC))
        .min(until)
}

/// Appends one option (code, length, data) to `buf` at `offset`.
/// Moves the offset to the end of the option appended.
fn put_option(buf: &mut [u8], offset: &mut usize, code: u8, data: &[u8]) {
    let len = data.len();

    debug_assert!(
        len <= u8::MAX as usize && *offset + len + 2 <= buf.len(),
        "buffer must fit the option"
    );

    buf[*offset] = code;
    buf[*offset + 1] = len as u8;
    buf[*offset + 2..*offset + 2 + len].copy_from_slice(data);
    *offset += 2 + len;
}

/// Yields (code, data).
/// Skips Pad (0) and stops at End (255) or a truncated option.
fn options(mut data: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    core::iter::from_fn(move || {
        while let [OPT_PAD, rest @ ..] = data {
            data = rest;
        }

        match data {
            [] | [OPT_END, ..] => None,
            [code, len, rest @ ..] => {
                let (value, rest) = rest.split_at_checked(*len as usize)?;
                data = rest;
                Some((*code, value))
            }
            _ => None,
        }
    })
}

/// Returns the value of the option with the given code, if present.
fn find_option(opts: &[u8], code: u8) -> Option<&[u8]> {
    options(opts).find(|&(c, _)| c == code).map(|(_, d)| d)
}

/// Returns the value of the option with the given code as a const-sized array, if present.
fn find_option_array<const N: usize>(opts: &[u8], code: u8) -> Option<[u8; N]> {
    find_option(opts, code)?.first_chunk::<N>().copied()
}

/// Builds a broadcast DHCP message and returns the frame length.
#[allow(clippy::too_many_arguments)]
fn build_frame(
    buf: &mut [u8],
    interface: &Interface,
    msg_type: u8,
    xid: u32,
    dst_mac: MacAddr,
    dst_ip: Ipv4Addr,
    ciaddr: Ipv4Addr,
    requested_ip: Option<Ipv4Addr>,
    server_id: Option<Ipv4Addr>,
) -> usize {
    debug_assert!(buf.len() >= MAX_FRAME, "buffer too small");

    let dhcp_start = EthernetHeader::SIZE + Ipv4Header::SIZE + UdpHeader::SIZE;
    let mut offset = dhcp_start;

    let mut bootp = BootpHeader::new_request();

    // copy our MAC address into the header
    bootp.chaddr[..6].copy_from_slice(&interface.mac.0);

    // set the broadcast flag if we don't have an IP address
    if ciaddr == Ipv4Addr::UNSPECIFIED {
        bootp.flags = Be::new(0x8000u16);
    } else {
        bootp.ciaddr = ciaddr;
    }

    // set the transaction ID
    bootp.xid = Be::new(xid);

    bootp.append_to(buf, &mut offset);

    buf[offset..offset + MAGIC_COOKIE.len()].copy_from_slice(&MAGIC_COOKIE);
    offset += MAGIC_COOKIE.len();

    put_option(buf, &mut offset, OPT_MESSAGE_TYPE, &[msg_type]);
    put_option(
        buf,
        &mut offset,
        OPT_PARAM_LIST,
        &[
            OPT_SUBNET_MASK,
            OPT_ROUTER,
            OPT_LEASE_TIME,
            OPT_SERVER_ID,
            OPT_RENEWAL_TIME,
            OPT_REBINDING_TIME,
        ],
    );

    if let (Some(ip), Some(id)) = (requested_ip, server_id) {
        put_option(buf, &mut offset, OPT_REQUESTED_IP, &ip.0);
        put_option(buf, &mut offset, OPT_SERVER_ID, &id.0);
    }

    buf[offset] = OPT_END;
    offset += 1;

    let dhcp_size = offset - dhcp_start;
    let mut header_offset = 0;

    EthernetHeader::new(dst_mac, interface.mac, EtherType::Ipv4).append_to(buf, &mut header_offset);

    Ipv4Header::new(ciaddr, dst_ip, Ipv4Proto::Udp, UdpHeader::SIZE + dhcp_size)
        .add_checksum()
        .append_to(buf, &mut header_offset);

    // always send from client (68) to server (67)
    // UDP checksum is ignored for DHCP, no need to calculate
    UdpHeader::new(CLIENT_PORT, SERVER_PORT, dhcp_size).append_to(buf, &mut header_offset);

    offset
}

fn transmit(fd: Fd, interface: &Interface, buf: &[u8]) {
    send(
        fd,
        buf,
        SockAddr::link(Some(interface.id), Some(EtherType::Ipv4 as u16)),
    )
    .unwrap_or_else(|e| exit_with_msg(e));
}

fn new_xid() -> u32 {
    let mut bytes = [0u8; 4];
    random(&mut bytes).unwrap_or_else(|e| exit_with_msg(e));
    u32::from_ne_bytes(bytes)
}

fn apply(interface: &Interface, offer: &Offer) {
    interface_set_ipv4(interface.id, offer.addr, offer.prefix_len)
        .unwrap_or_else(|e| exit_with_msg(e));

    if let Some(router) = offer.router {
        route_add(
            &RouteEntry {
                dest_ip: Ipv4Addr::UNSPECIFIED,
                prefix_len: 0,
                gateway: Some(router),
                interface_id: interface.id,
                metric: DHCP_ROUTE_METRIC,
                owner: RouteOwner::Dhcp,
                kind: RouteKind::Unicast,
                pref_src: None,
            },
            RouteFlag::ROUTE_REPLACE,
        )
        .unwrap_or_else(|e| exit_with_msg(e))
    }
}

fn unapply(interface: &Interface, last_offer: &Option<Offer>) {
    if let Some(last_offer) = last_offer
        && let Some(router) = last_offer.router
    {
        match route_remove(&RouteEntry {
            dest_ip: Ipv4Addr::UNSPECIFIED,
            prefix_len: 0,
            gateway: Some(router),
            interface_id: interface.id,
            metric: DHCP_ROUTE_METRIC,
            owner: RouteOwner::Dhcp,
            kind: RouteKind::Unicast,
            pref_src: None,
        }) {
            Ok(_) | Err(SysError::NoEntry) => {}
            Err(e) => exit_with_msg(e),
        }
    }

    interface_set_ipv4(interface.id, Ipv4Addr::UNSPECIFIED, 0).unwrap_or_else(|e| exit_with_msg(e));
}

/// Parses a full ethernet frame and returns BootpHeader and DHCP data if valid and addressed to us.
/// DHCP data skips over the magic cookie.
fn parse_reply<'a>(frame: &'a [u8], interface: &Interface) -> Option<(&'a BootpHeader, &'a [u8])> {
    // packet socket already binds to this interface and for IPv4 EtherType.
    let (_eth, rest) = EthernetHeader::from_bytes_with_rest(frame)?;

    let (ip, rest) = Ipv4Header::from_bytes_with_rest(rest)?;
    if ip.proto() != Ipv4Proto::Udp {
        return None;
    }

    // check udp destination port
    let (udp, rest) = UdpHeader::from_bytes_with_rest(rest)?;
    if udp.dest_port() != CLIENT_PORT {
        return None;
    }

    // check bootp is a reply and client MAC matches
    let (bootp, mut data) = BootpHeader::from_bytes_with_rest(rest)?;
    if bootp.op != 2 || bootp.chaddr[..6] != interface.mac.0 {
        return None;
    }

    // check and strip cookie
    data = data.strip_prefix(&MAGIC_COOKIE)?;

    Some((bootp, data))
}

fn parse_offer(header: &BootpHeader, data: &[u8]) -> Option<Offer> {
    let prefix_len = find_option_array(data, OPT_SUBNET_MASK)
        .map(Ipv4Addr)
        .and_then(Ipv4Addr::mask_to_prefix_len)?;

    let server_id = find_option_array(data, OPT_SERVER_ID).map(Ipv4Addr)?;

    let router = find_option_array(data, OPT_ROUTER).map(Ipv4Addr);

    let lease_secs = find_option_array(data, OPT_LEASE_TIME).map(u32::from_be_bytes)?;

    let t1_secs = find_option_array(data, OPT_RENEWAL_TIME)
        .map(u32::from_be_bytes)
        .unwrap_or(lease_secs / 2);

    let t2_secs = find_option_array(data, OPT_REBINDING_TIME)
        .map(u32::from_be_bytes)
        .unwrap_or(lease_secs / 8 * 7);

    Some(Offer {
        addr: header.yiaddr,
        prefix_len,
        server_id,
        router,
        lease_secs,
        t1_secs,
        t2_secs,
    })
}

#[unsafe(no_mangle)]
fn main(args: Args) {
    let dev = args.get_str(1).unwrap_or_else(|| exit_with_msg(USAGE));
    let interface = match resolve_interface(dev) {
        Ok(Some(interface)) => interface,
        Ok(None) => exit_with_msg(format_args!("interface not found: {dev}")),
        Err(e) => exit_with_msg(e),
    };

    let fd = socket(
        SocketType::Packet,
        SockAddr::link(Some(interface.id), Some(EtherType::Ipv4 as u16)),
    )
    .unwrap_or_else(|e| exit_with_msg(e));

    let mut client = Client {
        fd,
        interface,
        state: State::Selecting,
        xid: new_xid(),
        attempts: 0,
        deadline: 0,
        sent_at: 0,
        t1_at: 0,
        t2_at: 0,
        expires_at: 0,
        offer: None,
        server_mac: MacAddr::UNSPECIFIED,
    };

    client.discover(uptime());

    let mut buf = [0u8; MAX_FRAME];

    loop {
        ioctl(
            fd,
            Ioctl::SOCKET_SET_RECV_TIMEOUT,
            client.deadline.saturating_sub(uptime()).max(1),
        )
        .unwrap_or_else(|e| exit_with_msg(e));

        match receive(fd, &mut buf) {
            Ok((len, SockAddr::Link { mac, .. })) => client.on_reply(&buf[..len], mac),
            Ok((_, SockAddr::Inet { .. })) => unreachable!("socket must have a link address"),
            Err(SysError::TimedOut) => client.on_timeout(uptime()),
            Err(e) => exit_with_msg(e),
        };
    }
}
