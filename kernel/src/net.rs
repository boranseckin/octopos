use core::slice;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::e1000;
use crate::proc;
use crate::spinlock::SpinLock;

static NET_CONFIG: SpinLock<NetConfig> = SpinLock::new(
    NetConfig {
        mac: MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
        // TODO: use dhcp
        ip: Some(Ipv4Addr([192, 168, 10, 2])),
    },
    "net_config",
);

/// Network configuration, including our MAC address and optionally our IP address (if configured).
#[derive(Debug)]
struct NetConfig {
    mac: MacAddr,
    ip: Option<Ipv4Addr>,
}

/// Trait for integer types that can be converted to/from network byte order (big-endian).
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

/// Wrapper type for network byte order (big-endian) integers, providing methods to convert to/from
/// host byte order.
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

/// MAC Address
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct MacAddr([u8; 6]);

impl core::fmt::Display for MacAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

/// IPv4 Address
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct Ipv4Addr([u8; 4]);

impl core::fmt::Display for Ipv4Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
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
    dst: MacAddr,
    /// Source MAC Address
    src: MacAddr,
    /// Type
    ether_type: Be<u16>,
}

impl EthernetHeader {
    const SIZE: usize = size_of::<Self>();

    fn ether_type(&self) -> EtherType {
        EtherType::from(self.ether_type)
    }

    fn new_reply(request: &Self, our_mac: MacAddr) -> Self {
        Self {
            dst: request.src,
            src: our_mac,
            ether_type: request.ether_type,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
enum ArpHType {
    Ethernet = 1,
    Unknown = u16::MAX,
}

impl From<Be<u16>> for ArpHType {
    fn from(value: Be<u16>) -> Self {
        match value.get() {
            1 => Self::Ethernet,
            _ => Self::Unknown,
        }
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
    sha: MacAddr,
    /// Sender Protocol Address
    spa: Ipv4Addr,
    /// Target Hardware Address
    tha: MacAddr,
    /// Target Protocol Address
    tpa: Ipv4Addr,
}

impl ArpPacket {
    const SIZE: usize = size_of::<Self>();

    fn htype(&self) -> ArpHType {
        ArpHType::from(self.htype)
    }

    fn op(&self) -> ArpOp {
        ArpOp::from(self.op)
    }

    fn new_reply(request: &Self, our_mac: MacAddr, our_ip: Ipv4Addr) -> Self {
        Self {
            htype: request.htype,
            ptype: request.ptype,
            hlen: request.hlen,
            plen: request.plen,
            op: Be::new(ArpOp::Response as u16),
            sha: our_mac,
            spa: our_ip,
            tha: request.sha,
            tpa: request.spa,
        }
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
    src: Ipv4Addr,
    /// Destination IP Address
    dst: Ipv4Addr,
}

impl Ipv4Header {
    const SIZE: usize = size_of::<Self>();

    fn proto(&self) -> Ipv4Proto {
        Ipv4Proto::from(self.proto)
    }

    fn new_reply(request: &Self, payload_len: usize) -> Self {
        Self {
            ver_ihl: request.ver_ihl,
            tos: 0,
            len: Be::new((Self::SIZE + payload_len) as u16),
            id: Be::new(0),
            off: Be::new(0),
            ttl: 64,
            proto: request.proto,
            sum: Be::new(0), // to be filled later
            src: request.dst,
            dst: request.src,
        }
    }

    fn calculate_checksum(&self) -> u16 {
        let mut header = *self;
        header.sum = Be::new(0);
        internet_checksum(header.as_bytes())
    }

    fn add_checksum(mut self) -> Self {
        self.sum = Be::new(self.calculate_checksum());
        self
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

    fn new_echo_reply(request: &IcmpHeader) -> Self {
        Self {
            r#type: IcmpType::EchoReply as u8,
            code: 0,
            sum: Be::new(0), // to be filled later
            rest: request.rest,
        }
    }

    fn calculate_checksum(&self, data: &[u8]) -> u16 {
        // TODO: do this without vec
        let mut packet = Vec::with_capacity(Self::SIZE + data.len());
        packet.extend_from_slice(self.as_bytes());
        packet.extend_from_slice(data);
        internet_checksum(&packet)
    }

    fn add_checksum(mut self, data: &[u8]) -> Self {
        self.sum = Be::new(self.calculate_checksum(data));
        self
    }
}

/// Trait for parsing and serializing network headers.
///
/// # Safety
/// Implementors must ensure that the struct is `repr(C, packed)` and contains only plain data
/// fields, so that it can be safely transmuted to/from a byte slice without violating Rust's
/// aliasing or alignment rules.
unsafe trait NetworkHeader: Sized {
    /// Parses a header from the start of the given byte slice, returning `None` if the data is too
    /// short.
    fn from_bytes(data: &[u8]) -> Option<&Self> {
        if data.len() < size_of::<Self>() {
            return None;
        }

        // # Safety: the data is long enough, and the alignment is handled by the invariant of this
        // trait that the struct is `repr(C, packed)` with only plain data fields.
        Some(unsafe { &*(data.as_ptr() as *const Self) })
    }

    /// Parses a header from the start of the given byte slice, returning the header and a slice of
    /// the remaining data after the header, or `None` if the data is too short.
    fn from_bytes_with_rest(data: &[u8]) -> Option<(&Self, &[u8])> {
        let header = Self::from_bytes(data)?;
        let rest = &data[size_of::<Self>()..];
        Some((header, rest))
    }

    /// Returns a byte slice view of this header, for use in checksum calculations and serialization.
    fn as_bytes(&self) -> &[u8] {
        // # Safety: the struct is `repr(C, packed)` with only plain data fields, so it can be
        // safe to view it as a byte slice of the appropriate length.
        unsafe { slice::from_raw_parts(self as *const Self as *const u8, size_of::<Self>()) }
    }

    /// Appends the byte representation of this header to the given buffer at the specified offset,
    /// and advances the offset by the size of this header.
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

/// Handles an incoming ARP packet by parsing the header and, if it is a request for our IP, sending
/// a reply with our MAC address.
fn handle_arp(req_eth: &EthernetHeader, req_data: &[u8]) {
    let Some(req_arp) = ArpPacket::from_bytes(req_data) else {
        return;
    };

    if req_arp.op() == ArpOp::Request
        && req_arp.htype() == ArpHType::Ethernet
        && EtherType::from(req_arp.ptype) == EtherType::Ipv4
    {
        let ip = {
            let guard = NET_CONFIG.lock();
            let Some(ip) = guard.ip else { return };
            ip
        };

        if req_arp.tpa == ip {
            send_arp_reply(req_eth, req_arp);
        }
    }
}

/// Sends an ARP Reply in response to the given ARP Request, swapping source/destination addresses
/// and filling in our MAC/IP as needed.
fn send_arp_reply(req_eth: &EthernetHeader, req_arp: &ArpPacket) {
    let mut packet = [0u8; EthernetHeader::SIZE + ArpPacket::SIZE];
    let mut offset = 0;

    let (mac, ip) = {
        let guard = NET_CONFIG.lock();
        let Some(ip) = guard.ip else { return };
        (guard.mac, ip)
    };

    EthernetHeader::new_reply(req_eth, mac).append_to(&mut packet, &mut offset);
    ArpPacket::new_reply(req_arp, mac, ip).append_to(&mut packet, &mut offset);

    e1000::transmit(&packet).expect("failed to send arp reply");
}

/// Handles an incoming IPv4 packet by parsing the header and dispatching to the appropriate
/// protocol handler.
fn handle_ipv4(req_eth: &EthernetHeader, packet: &[u8]) {
    let Some((req_ipv4, req_data)) = Ipv4Header::from_bytes_with_rest(packet) else {
        return;
    };

    // Remove extra padding bytes added by the NIC, if any.
    let ipv4_payload_len = req_ipv4.len.get() as usize - Ipv4Header::SIZE;
    let req_data = &req_data[..ipv4_payload_len.min(req_data.len())];

    match req_ipv4.proto() {
        Ipv4Proto::ICMP => {
            if let Some((req_icmp, req_data)) = IcmpHeader::from_bytes_with_rest(req_data) {
                if req_icmp.r#type() == IcmpType::EchoRequest {
                    send_icmp_reply(req_eth, req_ipv4, req_icmp, req_data);
                }
            }
        }
        Ipv4Proto::UDP => {}
        Ipv4Proto::Unknown => {}
    }
}

/// Sends an ICMP Echo Reply in response to the given Echo Request, swapping source/destination
/// addresses and recalculating checksums as needed.
///
/// This function uses heap allocation to build the packet, since the data payload size is variable.
fn send_icmp_reply(
    req_eth: &EthernetHeader,
    req_ipv4: &Ipv4Header,
    req_icmp: &IcmpHeader,
    req_data: &[u8],
) {
    let mac = {
        let guard = NET_CONFIG.lock();
        guard.mac
    };

    // pre-allocate packet buffer for Eth + (IP + ICMP)
    let mut packet = Vec::with_capacity(EthernetHeader::SIZE + req_ipv4.len.get() as usize);

    let eth = EthernetHeader::new_reply(req_eth, mac);
    let icmp = IcmpHeader::new_echo_reply(req_icmp).add_checksum(req_data);
    let ipv4 = Ipv4Header::new_reply(req_ipv4, IcmpHeader::SIZE + req_data.len()).add_checksum();

    packet.extend_from_slice(eth.as_bytes());
    packet.extend_from_slice(ipv4.as_bytes());
    packet.extend_from_slice(icmp.as_bytes());
    packet.extend_from_slice(req_data);

    e1000::transmit(&packet).expect("failed to send icmp reply");
}

/// Computes the one's-complement Internet checksum used by IP and ICMP (RFC 1071).
///
/// The algorithm sums all 16-bit big-endian words, folds any carry bits back into the low 16 bits,
/// then returns the one's complement.
///
/// Before calling this, zero out the checksum field in the header so it is not included.
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

/// Receives a packet, parses the Ethernet header, and dispatches to the appropriate handler based
/// on the EtherType.
fn rx(packet: Box<[u8]>) {
    let Some((eth, data)) = EthernetHeader::from_bytes_with_rest(&packet) else {
        return;
    };

    match eth.ether_type() {
        EtherType::Arp => handle_arp(eth, data),
        EtherType::Ipv4 => handle_ipv4(eth, data),
        EtherType::Unknown => {}
    }
}

/// Kernel thread that continuously receives packets from the e1000 driver and processes them.
fn net_thread() {
    loop {
        let data = e1000::receive();
        rx(data);
    }
}

/// Initializes the network stack by spawning a kernel thread to handle incoming packets.
pub fn init() {
    proc::spawn_kernel_thread(net_thread, "net");
}
