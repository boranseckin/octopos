use alloc::vec::Vec;

use crate::net::ipv4::Ipv4Proto;
use crate::net::{self, Be, Ipv4Addr, NetError, NetworkHeader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IcmpType {
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
pub struct IcmpHeader {
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

    pub fn r#type(&self) -> IcmpType {
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
        net::internet_checksum(&packet)
    }

    fn add_checksum(mut self, data: &[u8]) -> Self {
        self.sum = Be::new(self.calculate_checksum(data));
        self
    }
}

unsafe impl NetworkHeader for IcmpHeader {}

/// Sends an ICMP Echo Reply in response to the given ICMP Echo Request.
///
/// This function uses heap allocation to build the packet, since the data payload size is variable.
pub fn echo_reply(
    dest_ip: Ipv4Addr,
    req_icmp: &IcmpHeader,
    req_data: &[u8],
) -> Result<(), NetError> {
    let mut packet = Vec::with_capacity(IcmpHeader::SIZE + req_data.len());

    let icmp = IcmpHeader::new_echo_reply(req_icmp).add_checksum(req_data);

    packet.extend_from_slice(icmp.as_bytes());
    packet.extend_from_slice(req_data);

    log!(net::transmit(dest_ip, Ipv4Proto::ICMP, &packet))
}
