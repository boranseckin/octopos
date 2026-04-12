use core::slice;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::e1000;
use crate::proc;
use crate::spinlock::SpinLock;

const ETH_TYPE_ARP: u16 = 0x0806;
const ETH_TYPE_IP4: u16 = 0x0800;

const ARP_HTYPE_ETH: u16 = 0x1;
const ARP_OP_REQ: u16 = 0x1;
const ARP_OP_RES: u16 = 0x2;

const IPV4_PROTO_ICMP: u8 = 1;
const IPV4_PROTO_UDP: u8 = 17;

const ICMP_TYPE_ECHO_REQ: u8 = 8;
const ICMP_TYPE_ECHO_RES: u8 = 0;

static NET_CONFIG: SpinLock<NetConfig> = SpinLock::new(
    NetConfig {
        mac: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        ip: Some([192, 168, 10, 2]),
    },
    "net_config",
);

#[derive(Debug)]
struct NetConfig {
    mac: [u8; 6],
    ip: Option<[u8; 4]>,
}

/// Ethernet Frame Header
/// https://en.wikipedia.org/wiki/Ethernet_frame#Ethernet_II
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct EthernetHeader {
    /// Destination MAC Address
    dst: [u8; 6],
    /// Source MAC Address
    src: [u8; 6],
    /// Type
    eth_type: u16,
}

impl EthernetHeader {
    const SIZE: usize = size_of::<Self>();
}

/// Address Resolution Protocol (ARP) Packet
/// https://datatracker.ietf.org/doc/html/rfc826
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct ArpPacket {
    /// Hardware Type
    htype: u16,
    /// Protocol Type
    ptype: u16,
    /// Hardware Address Length
    hlen: u8,
    /// Protocol Address Length
    plen: u8,
    /// Operation
    op: u16,
    /// Sender Hardware Address
    sha: [u8; 6],
    /// Sender Protocol Address
    spa: [u8; 4],
    /// Target Hardware Address
    tha: [u8; 6],
    /// Target Protocol Address
    tpa: [u8; 4],
}

impl ArpPacket {
    const SIZE: usize = size_of::<Self>();
}

/// IPv4 Header
/// https://en.wikipedia.org/wiki/IPv4#Packet_structure
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct Ipv4Header {
    /// Version and Internet Header Length
    ver_ihl: u8,
    /// Type of Service
    tos: u8,
    /// Total Length
    len: u16,
    /// Identification
    id: u16,
    /// Flags and Fragment Offset
    off: u16,
    /// Time to Live
    ttl: u8,
    /// Protocol
    proto: u8,
    /// Checksum
    sum: u16,
    /// Source IP Address
    src: [u8; 4],
    /// Destination IP Address
    dst: [u8; 4],
}

impl Ipv4Header {
    const SIZE: usize = size_of::<Self>();
}

/// ICMP Header
/// https://en.wikipedia.org/wiki/Internet_Control_Message_Protocol#Header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct IcmpHeader {
    /// Type
    typ: u8,
    /// Code
    code: u8,
    /// Checksum
    sum: u16,
    /// Rest of Header (varies by type/code)
    rest: u32,
}

impl IcmpHeader {
    const SIZE: usize = size_of::<Self>();
}

unsafe trait NetworkHeader: Sized {
    fn from_bytes(data: &[u8]) -> Option<&Self> {
        if data.len() < size_of::<Self>() {
            return None;
        }

        // # Safety: the data is long enough, and the alignment is handled by repr(packed).
        Some(unsafe { &*(data.as_ptr() as *const Self) })
    }

    fn as_bytes(&self) -> &[u8] {
        unsafe { slice::from_raw_parts(self as *const Self as *const u8, size_of::<Self>()) }
    }

    fn append_to(&self, buf: &mut [u8], offset: &mut usize) {
        let bytes = self.as_bytes();
        let len = bytes.len();

        buf[*offset..*offset + len].copy_from_slice(bytes);

        *offset += len;
    }
}

unsafe impl NetworkHeader for EthernetHeader {}
unsafe impl NetworkHeader for ArpPacket {}
unsafe impl NetworkHeader for Ipv4Header {}
unsafe impl NetworkHeader for IcmpHeader {}

fn handle_arp(packet: &[u8]) {
    let Some(arp) = ArpPacket::from_bytes(packet) else {
        return;
    };

    let op = u16::from_be(arp.op);
    let htype = u16::from_be(arp.htype);
    let ptype = u16::from_be(arp.ptype);

    if op == ARP_OP_REQ && htype == ARP_HTYPE_ETH && ptype == ETH_TYPE_IP4 {
        let ip = {
            let guard = NET_CONFIG.lock();
            guard.ip.unwrap()
        };

        if arp.tpa == ip {
            send_arp_reply(arp);
        }
    }
}

fn send_arp_reply(request: &ArpPacket) {
    let mut packet = [0u8; EthernetHeader::SIZE + ArpPacket::SIZE];
    let mut offset = 0;

    let (mac, ip) = {
        let guard = NET_CONFIG.lock();
        (guard.mac, guard.ip)
    };

    let eth = EthernetHeader {
        dst: request.sha,
        src: mac,
        eth_type: ETH_TYPE_ARP.to_be(),
    };

    let arp = ArpPacket {
        htype: ARP_HTYPE_ETH.to_be(),
        ptype: ETH_TYPE_IP4.to_be(),
        hlen: 6,
        plen: 4,
        op: ARP_OP_RES.to_be(),
        sha: mac,
        spa: ip.unwrap(),
        tha: request.sha,
        tpa: request.spa,
    };

    eth.append_to(&mut packet, &mut offset);
    arp.append_to(&mut packet, &mut offset);

    e1000::transmit(&packet).unwrap();
}

fn handle_ipv4(eth_req: &EthernetHeader, packet: &[u8]) {
    let Some(ipv4) = Ipv4Header::from_bytes(packet) else {
        return;
    };

    match ipv4.proto {
        IPV4_PROTO_ICMP => {
            let icmp_data = &packet[Ipv4Header::SIZE..];
            if let Some(icmp) = IcmpHeader::from_bytes(icmp_data) {
                if icmp.typ == ICMP_TYPE_ECHO_REQ {
                    send_icmp_reply(eth_req.src, ipv4, icmp_data);
                }
            }
        }
        IPV4_PROTO_UDP => {}
        _ => {}
    }
}

fn send_icmp_reply(dest_mac: [u8; 6], request_ip: &Ipv4Header, request_icmp: &[u8]) {
    let mac = {
        let guard = NET_CONFIG.lock();
        guard.mac
    };

    // pre-allocate packet buffer for Eth + (IP + ICMP)
    let mut packet =
        Vec::with_capacity(EthernetHeader::SIZE + u16::from_be(request_ip.len) as usize);

    let eth = EthernetHeader {
        dst: dest_mac,
        src: mac,
        eth_type: ETH_TYPE_IP4.to_be(),
    };

    let mut ip = Ipv4Header {
        ver_ihl: (4 << 4) | (Ipv4Header::SIZE as u8 / 4),
        tos: 0,
        len: request_ip.len,
        id: 0,
        off: 0,
        ttl: 64,
        proto: IPV4_PROTO_ICMP,
        sum: 0, // to be filled later
        src: request_ip.dst,
        dst: request_ip.src,
    };
    ip.sum = internet_checksum(ip.as_bytes()).to_be();

    packet.extend_from_slice(eth.as_bytes());
    packet.extend_from_slice(ip.as_bytes());

    let icmp = IcmpHeader {
        typ: ICMP_TYPE_ECHO_RES,
        code: 0,
        sum: 0, // to be filled later
        rest: IcmpHeader::from_bytes(request_icmp).unwrap().rest,
    };

    let icmp_start = packet.len();

    // add icmp header and the data from the icmp request
    packet.extend_from_slice(icmp.as_bytes());
    packet.extend_from_slice(&request_icmp[IcmpHeader::SIZE..]);

    // calculate checksum over the whole icmp packet and patch the fields
    let icmp_checksum = internet_checksum(&packet[icmp_start..]);
    let sum_bytes = icmp_checksum.to_be_bytes();
    packet[icmp_start + 2] = sum_bytes[0];
    packet[icmp_start + 3] = sum_bytes[1];

    e1000::transmit(&packet).unwrap();
}

/// Computes the one's-complement Internet checksum used by IP and ICMP (RFC 1071).
///
/// The algorithm sums all 16-bit big-endian words, folds any carry bits back
/// into the low 16 bits, then returns the one's complement. Before calling
/// this, zero out the checksum field in the header so it is not included.
fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    let mut chunks = data.chunks_exact(2);
    for chunk in &mut chunks {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    // If there is a trailing odd byte, pad it with a zero on the right.
    if let Some(&byte) = chunks.remainder().first() {
        sum += (byte as u32) << 8;
    }

    // Fold the 32-bit accumulator into 16 bits.
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }

    !(sum as u16)
}

fn rx(packet: Box<[u8]>) {
    let Some(eth) = EthernetHeader::from_bytes(&packet) else {
        return;
    };

    let data_packet = &packet[size_of::<EthernetHeader>()..];

    match u16::from_be(eth.eth_type) {
        ETH_TYPE_ARP => handle_arp(data_packet),
        ETH_TYPE_IP4 => handle_ipv4(eth, data_packet),
        _ => {}
    }
}

fn thread() {
    loop {
        let data = e1000::receive();
        rx(data);
    }
}

pub fn init() {
    proc::spawn_kernel_thread(thread, "net");
}
