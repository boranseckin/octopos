use alloc::vec::Vec;

use crate::e1000;
use crate::net::eth::EthernetHeader;
use crate::net::ipv4::Ipv4Header;
use crate::net::{Be, NET_CONFIG, NetworkHeader, internet_checksum};

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
        internet_checksum(&packet)
    }

    fn add_checksum(mut self, data: &[u8]) -> Self {
        self.sum = Be::new(self.calculate_checksum(data));
        self
    }
}

unsafe impl NetworkHeader for IcmpHeader {}

/// Sends an ICMP Echo Reply in response to the given Echo Request, swapping source/destination
/// addresses and recalculating checksums as needed.
///
/// This function uses heap allocation to build the packet, since the data payload size is variable.
pub fn send_icmp_reply(
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
    let mut packet = Vec::with_capacity(EthernetHeader::SIZE + req_ipv4.len() as usize);

    let eth = EthernetHeader::new_reply(req_eth, mac);
    let icmp = IcmpHeader::new_echo_reply(req_icmp).add_checksum(req_data);
    let ipv4 = Ipv4Header::new_reply(req_ipv4, IcmpHeader::SIZE + req_data.len()).add_checksum();

    packet.extend_from_slice(eth.as_bytes());
    packet.extend_from_slice(ipv4.as_bytes());
    packet.extend_from_slice(icmp.as_bytes());
    packet.extend_from_slice(req_data);

    e1000::transmit(&packet).expect("failed to send icmp reply");
}
