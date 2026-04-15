use crate::net::{Be, MacAddr, NetworkHeader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum EtherType {
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
pub struct EthernetHeader {
    /// Destination MAC Address
    dst: MacAddr,
    /// Source MAC Address
    src: MacAddr,
    /// Type
    ether_type: Be<u16>,
}

impl EthernetHeader {
    pub const SIZE: usize = size_of::<Self>();

    pub fn ether_type(&self) -> EtherType {
        EtherType::from(self.ether_type)
    }

    pub fn new(dst: MacAddr, src: MacAddr, ether_type: EtherType) -> Self {
        Self {
            dst,
            src,
            ether_type: Be::new(ether_type as u16),
        }
    }

    pub fn new_reply(request: &Self, src: MacAddr) -> Self {
        Self {
            dst: request.src,
            src,
            ether_type: request.ether_type,
        }
    }
}

unsafe impl NetworkHeader for EthernetHeader {}
