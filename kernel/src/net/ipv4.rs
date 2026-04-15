use crate::net::eth::EthernetHeader;
use crate::net::icmp::{self, IcmpHeader, IcmpType};
use crate::net::{self, Be, Ipv4Addr, NetworkHeader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Ipv4Proto {
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
    dst: Ipv4Addr,
}

impl Ipv4Header {
    const SIZE: usize = size_of::<Self>();

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
            dst,
        }
    }

    pub fn new_reply(request: &Self, payload_len: usize) -> Self {
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
        net::internet_checksum(header.as_bytes())
    }

    pub fn add_checksum(mut self) -> Self {
        self.sum = Be::new(self.calculate_checksum());
        self
    }
}

unsafe impl NetworkHeader for Ipv4Header {}

/// Handles an incoming IPv4 packet by parsing the header and dispatching to the appropriate
/// protocol handler.
pub fn handle_ipv4(req_eth: &EthernetHeader, packet: &[u8]) {
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
                    icmp::echo_reply(req_eth, req_ipv4, req_icmp, req_data);
                }
            }
        }
        Ipv4Proto::UDP => {}
        Ipv4Proto::Unknown => {}
    }
}
