use crate::net::interface::InterfaceId;
use crate::net::udp;
use crate::net::{self, Be, Ipv4Addr, NetError, NetworkHeader};
use crate::net::{icmp, internet_checksum};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Ipv4Proto {
    Icmp = 1,
    Udp = 17,
    Unknown = u8::MAX,
}

impl From<u8> for Ipv4Proto {
    fn from(value: u8) -> Self {
        match value {
            1 => Self::Icmp,
            17 => Self::Udp,
            _ => Self::Unknown,
        }
    }
}

/// IPv4 Header
/// https://en.wikipedia.org/wiki/IPv4#Packet_structure
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Ipv4Header {
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
    dest: Ipv4Addr,
}

impl Ipv4Header {
    pub const SIZE: usize = size_of::<Self>();

    fn proto(&self) -> Ipv4Proto {
        Ipv4Proto::from(self.proto)
    }

    pub fn len(&self) -> u16 {
        self.len.get()
    }

    pub fn new(src: Ipv4Addr, dst: Ipv4Addr, proto: Ipv4Proto, payload_len: usize) -> Self {
        Self {
            ver_ihl: (4 << 4) | (Self::SIZE as u8 / 4),
            tos: 0,
            len: Be::new((Self::SIZE + payload_len) as u16),
            id: Be::new(0),
            off: Be::new(0),
            ttl: 64,
            proto: proto as u8,
            sum: Be::new(0), // to be filled later
            src,
            dest: dst,
        }
    }

    fn calculate_checksum(&self) -> u16 {
        let mut header = *self;
        header.sum = Be::new(0);
        net::internet_checksum(&[header.as_bytes()])
    }

    pub fn add_checksum(mut self) -> Self {
        self.sum = Be::new(self.calculate_checksum());
        self
    }
}

unsafe impl NetworkHeader for Ipv4Header {}

/// Handles an incoming IPv4 packet by parsing the header and dispatching to the appropriate
/// protocol handler.
pub fn handle_ipv4(interface_id: InterfaceId, packet: &[u8]) -> Result<(), NetError> {
    let Some((req_ipv4, req_data)) = Ipv4Header::from_bytes_with_rest(packet) else {
        err!(NetError::MalformedPacket)
    };

    let ver = req_ipv4.ver_ihl >> 4;
    let ihl = req_ipv4.ver_ihl & 0xf;

    // Only accept version 4 and 20-byte packets (ihl=5 * 4).
    if ver != 4 || ihl != 5 {
        err!(NetError::MalformedPacket);
    }

    // Make sure the length is greater than the header size and that the packet fits the length
    // spceified in the header.
    let total_len = req_ipv4.len() as usize;
    if total_len < Ipv4Header::SIZE || packet.len() < total_len {
        err!(NetError::MalformedPacket)
    }

    // Checksum control
    let header_without_sum = Ipv4Header {
        sum: Be::new(0),
        ..*req_ipv4
    };
    if internet_checksum(&[header_without_sum.as_bytes()]) != req_ipv4.sum.get() {
        err!(NetError::ChecksumFailed);
    }

    // Remove extra padding bytes added by the NIC, if any.
    let ipv4_payload_len = total_len - Ipv4Header::SIZE;
    let req_data = &req_data[..ipv4_payload_len.min(req_data.len())];

    match req_ipv4.proto() {
        Ipv4Proto::Icmp => log!(icmp::handle_icmp(req_ipv4.src, req_data)),
        Ipv4Proto::Udp => log!(udp::handle_udp(
            interface_id,
            req_ipv4.dest,
            req_ipv4.src,
            req_data
        )),
        Ipv4Proto::Unknown => Ok(()),
    }
}
