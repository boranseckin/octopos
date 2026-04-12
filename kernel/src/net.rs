use core::slice;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::e1000;
use crate::proc;
use crate::spinlock::SpinLock;

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

trait NetworkInt: Sized + Copy {
    fn to_be(self) -> Self;
    fn from_be(self) -> Self;
}

impl NetworkInt for u16 {
    fn to_be(self) -> Self {
        u16::to_be(self)
    }

    fn from_be(self) -> Self {
        u16::from_be(self)
    }
}

impl NetworkInt for u32 {
    fn to_be(self) -> Self {
        u32::to_be(self)
    }

    fn from_be(self) -> Self {
        u32::from_be(self)
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
struct Be<T: Copy>(T);

impl<T: NetworkInt> Be<T> {
    fn new(value: T) -> Self {
        Self(value.to_be())
    }

    fn get(self) -> T {
        self.0.from_be()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
enum EtherType {
    Ipv4 = 0x0800,
    Arp = 0x0806,
    Unknown = u16::MAX,
}

impl From<Be<u16>> for EtherType {
    fn from(value: Be<u16>) -> Self {
        match value.get() {
            0x0800 => Self::Ipv4,
            0x0806 => Self::Arp,
            _ => Self::Unknown,
        }
    }
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
    ether_type: Be<u16>,
}

impl EthernetHeader {
    const SIZE: usize = size_of::<Self>();

    fn ether_type(&self) -> EtherType {
        EtherType::from(self.ether_type)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
enum ArpOp {
    Request = 1,
    Response = 2,
    Unknown = u16::MAX,
}

impl From<Be<u16>> for ArpOp {
    fn from(value: Be<u16>) -> Self {
        match value.get() {
            1 => Self::Request,
            2 => Self::Response,
            _ => Self::Unknown,
        }
    }
}

/// Address Resolution Protocol (ARP) Packet
/// https://datatracker.ietf.org/doc/html/rfc826
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct ArpPacket {
    /// Hardware Type
    htype: Be<u16>,
    /// Protocol Type
    ptype: Be<u16>,
    /// Hardware Address Length
    hlen: u8,
    /// Protocol Address Length
    plen: u8,
    /// Operation
    op: Be<u16>,
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
    const HTYPE_ETH: u16 = 0x1;

    fn op(&self) -> ArpOp {
        ArpOp::from(self.op)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Ipv4Proto {
    ICMP = 1,
    UDP = 17,
    Unknown = u8::MAX,
}

impl From<u8> for Ipv4Proto {
    fn from(value: u8) -> Self {
        match value {
            1 => Self::ICMP,
            17 => Self::UDP,
            _ => Self::Unknown,
        }
    }
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
    len: Be<u16>,
    /// Identification
    id: Be<u16>,
    /// Flags and Fragment Offset
    off: Be<u16>,
    /// Time to Live
    ttl: u8,
    /// Protocol
    proto: u8,
    /// Checksum
    sum: Be<u16>,
    /// Source IP Address
    src: [u8; 4],
    /// Destination IP Address
    dst: [u8; 4],
}

impl Ipv4Header {
    const SIZE: usize = size_of::<Self>();

    fn proto(&self) -> Ipv4Proto {
        Ipv4Proto::from(self.proto)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum IcmpType {
    EchoReply = 0,
    EchoRequest = 8,
    Unknown = u8::MAX,
}

impl From<u8> for IcmpType {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::EchoReply,
            8 => Self::EchoRequest,
            _ => Self::Unknown,
        }
    }
}

/// ICMP Header
/// https://en.wikipedia.org/wiki/Internet_Control_Message_Protocol#Header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct IcmpHeader {
    /// Type
    r#type: u8,
    /// Code
    code: u8,
    /// Checksum
    sum: Be<u16>,
    /// Rest of Header (varies by type/code)
    rest: Be<u32>,
}

impl IcmpHeader {
    const SIZE: usize = size_of::<Self>();

    fn r#type(&self) -> IcmpType {
        IcmpType::from(self.r#type)
    }
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

    let op = arp.op();
    let htype = arp.htype.get();
    let ptype = arp.ptype.get();

    if op == ArpOp::Request && htype == ArpPacket::HTYPE_ETH && ptype == (EtherType::Ipv4 as u16) {
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
        ether_type: Be::new(EtherType::Arp as u16),
    };

    let arp = ArpPacket {
        htype: Be::new(ArpPacket::HTYPE_ETH),
        ptype: request.ptype,
        hlen: 6,
        plen: 4,
        op: Be::new(ArpOp::Response as u16),
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

    match ipv4.proto() {
        Ipv4Proto::ICMP => {
            let icmp_data = &packet[Ipv4Header::SIZE..];
            if let Some(icmp) = IcmpHeader::from_bytes(icmp_data) {
                if icmp.r#type() == IcmpType::EchoRequest {
                    send_icmp_reply(eth_req.src, ipv4, icmp_data);
                }
            }
        }
        Ipv4Proto::UDP => {}
        Ipv4Proto::Unknown => {}
    }
}

fn send_icmp_reply(dest_mac: [u8; 6], request_ip: &Ipv4Header, request_icmp: &[u8]) {
    let mac = {
        let guard = NET_CONFIG.lock();
        guard.mac
    };

    // pre-allocate packet buffer for Eth + (IP + ICMP)
    let mut packet = Vec::with_capacity(EthernetHeader::SIZE + request_ip.len.get() as usize);

    let eth = EthernetHeader {
        dst: dest_mac,
        src: mac,
        ether_type: Be::new(EtherType::Ipv4 as u16),
    };

    let mut ip = Ipv4Header {
        ver_ihl: (4 << 4) | (Ipv4Header::SIZE as u8 / 4),
        tos: 0,
        len: request_ip.len,
        id: Be::new(0),
        off: Be::new(0),
        ttl: 64,
        proto: Ipv4Proto::ICMP as u8,
        sum: Be::new(0), // to be filled later
        src: request_ip.dst,
        dst: request_ip.src,
    };
    ip.sum = Be::new(internet_checksum(ip.as_bytes()));

    packet.extend_from_slice(eth.as_bytes());
    packet.extend_from_slice(ip.as_bytes());

    let icmp = IcmpHeader {
        r#type: IcmpType::EchoReply as u8,
        code: 0,
        sum: Be::new(0), // to be filled later
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

    match eth.ether_type() {
        EtherType::Arp => handle_arp(data_packet),
        EtherType::Ipv4 => handle_ipv4(eth, data_packet),
        EtherType::Unknown => {}
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
