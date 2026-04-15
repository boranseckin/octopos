use crate::e1000;
use crate::net::EtherType;
use crate::net::{Be, EthernetHeader, Ipv4Addr, MacAddr, NET_CONFIG, NetworkHeader};

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

unsafe impl NetworkHeader for ArpPacket {}

/// Handles an incoming ARP packet by parsing the header and, if it is a request for our IP, sending
/// a reply with our MAC address.
pub fn handle_arp(req_eth: &EthernetHeader, req_data: &[u8]) {
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
