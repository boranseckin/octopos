use crate::net::{
    self, Be, EtherType, EthernetHeader, Ipv4Addr, MacAddr, NET_CONFIG, NetworkHeader,
};
use crate::spinlock::SpinLock;

const ARP_CACHE_SIZE: usize = 64;

pub static ARP_CACHE: SpinLock<ArpCache> = SpinLock::new(
    ArpCache {
        entries: [None; ARP_CACHE_SIZE],
        eviction_index: 0,
    },
    "arp_cache",
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArpCacheEntry {
    ip: Ipv4Addr,
    mac: MacAddr,
}

/// Simple ARP cache implementation with a fixed-size array and linear search.
#[derive(Debug)]
pub struct ArpCache {
    entries: [Option<ArpCacheEntry>; ARP_CACHE_SIZE],
    /// Index of the next entry to evict when the cache is full. Close to random eviction.
    eviction_index: usize,
}

impl ArpCache {
    /// Looks up the given IP address in the ARP cache and returns the corresponding MAC address if
    /// found.
    pub fn lookup(&self, ip: Ipv4Addr) -> Option<MacAddr> {
        self.entries
            .iter()
            .find_map(|entry| entry.filter(|e| e.ip == ip).map(|e| e.mac))
    }

    /// Inserts the given IP-MAC mapping into the ARP cache, evicting an existing entry if
    /// necessary.
    fn insert(&mut self, ip: Ipv4Addr, mac: MacAddr) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.is_none()) {
            *entry = Some(ArpCacheEntry { ip, mac });
        } else {
            // drop a "random" entry if the cache is full
            self.entries[self.eviction_index] = Some(ArpCacheEntry { ip, mac });
        }

        self.eviction_index = (self.eviction_index + 1) % ARP_CACHE_SIZE;
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

    fn new_request(target_ip: Ipv4Addr, src_mac: MacAddr, src_ip: Ipv4Addr) -> Self {
        Self {
            htype: Be::new(ArpHType::Ethernet as u16),
            ptype: Be::new(EtherType::Ipv4 as u16),
            hlen: 6,
            plen: 4,
            op: Be::new(ArpOp::Request as u16),
            sha: src_mac,
            spa: src_ip,
            tha: MacAddr([0; 6]),
            tpa: target_ip,
        }
    }

    fn new_reply(request: &Self, src_mac: MacAddr, src_ip: Ipv4Addr) -> Self {
        Self {
            htype: request.htype,
            ptype: request.ptype,
            hlen: request.hlen,
            plen: request.plen,
            op: Be::new(ArpOp::Response as u16),
            sha: src_mac,
            spa: src_ip,
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

    // we only support Ethernet+IPv4 ARP packets, so ignore anything else
    if req_arp.htype() != ArpHType::Ethernet || EtherType::from(req_arp.ptype) != EtherType::Ipv4 {
        return;
    }

    // we need our IP address to know whether to reply, so ignore ARP packets if we don't have one
    let ip = {
        let guard = NET_CONFIG.lock();
        let Some(ip) = guard.ip else { return };
        ip
    };

    // ignore ARP packets that aren't for our IP address
    if req_arp.tpa != ip {
        return;
    }

    match req_arp.op() {
        ArpOp::Request => {
            reply(req_eth, req_arp);
        }
        ArpOp::Response => {
            let mut cache = ARP_CACHE.lock();
            cache.insert(req_arp.spa, req_arp.sha);
        }
        ArpOp::Unknown => {}
    }
}

/// Sends an ARP Reply in response to the given ARP Request, swapping source/destination addresses
/// and filling in our MAC/IP as needed.
fn reply(req_eth: &EthernetHeader, req_arp: &ArpPacket) {
    let mut packet = [0u8; EthernetHeader::SIZE + ArpPacket::SIZE];
    let mut offset = 0;

    let (mac, ip) = {
        let guard = NET_CONFIG.lock();
        let Some(ip) = guard.ip else { return };
        (guard.mac, ip)
    };

    EthernetHeader::new_reply(req_eth, mac).append_to(&mut packet, &mut offset);
    ArpPacket::new_reply(req_arp, mac, ip).append_to(&mut packet, &mut offset);

    net::transmit(&packet);
}

pub fn request(dest_ip: Ipv4Addr) {
    let mut packet = [0u8; EthernetHeader::SIZE + ArpPacket::SIZE];
    let mut offset = 0;

    let (mac, ip) = {
        let guard = NET_CONFIG.lock();
        let Some(ip) = guard.ip else { return };
        (guard.mac, ip)
    };

    EthernetHeader::new(MacAddr::BROADCAST, mac, EtherType::Arp)
        .append_to(&mut packet, &mut offset);
    ArpPacket::new_request(dest_ip, mac, ip).append_to(&mut packet, &mut offset);

    net::transmit(&packet);
}
