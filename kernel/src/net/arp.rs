use crate::net::eth::{EtherType, EthernetHeader};
use crate::net::interface::{self, InterfaceId};
use crate::net::{Be, Ipv4Addr, MacAddr, NetError, NetworkHeader, OutgoingQueue};
use crate::spinlock::SpinLock;

const ARP_CACHE_SIZE: usize = 64;

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

static ARP_CACHE: SpinLock<ArpCache> = SpinLock::new(
    ArpCache {
        entries: [None; ARP_CACHE_SIZE],
        eviction_index: 0,
    },
    "arp_cache",
);

impl ArpCache {
    /// Looks up the given IP address in the ARP cache and returns the corresponding MAC address if
    /// found.
    pub fn lookup(ip: Ipv4Addr) -> Option<MacAddr> {
        ARP_CACHE
            .lock()
            .entries
            .iter()
            .find_map(|entry| entry.filter(|e| e.ip == ip).map(|e| e.mac))
    }

    /// Inserts the given IP-MAC mapping into the ARP cache, evicting an existing entry if
    /// necessary.
    fn insert(ip: Ipv4Addr, mac: MacAddr) {
        let mut cache = ARP_CACHE.lock();

        // check for existing mapping and update.
        // if doesn't exist, find an empty slot and insert.
        // if doesn't exist, evict a "random" entry and insert.
        if let Some(matching) = cache
            .entries
            .iter_mut()
            .find(|entry| entry.as_ref().is_some_and(|e| e.ip == ip))
        {
            matching.as_mut().unwrap().mac = mac;
        } else if let Some(entry) = cache.entries.iter_mut().find(|entry| entry.is_none()) {
            *entry = Some(ArpCacheEntry { ip, mac });
        } else {
            // drop a "random" entry if the cache is full
            // randomness comes from the randomness of network traffic
            let index = cache.eviction_index;
            cache.entries[index] = Some(ArpCacheEntry { ip, mac });
            cache.eviction_index = (cache.eviction_index + 1) % ARP_CACHE_SIZE;
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
pub fn handle_arp(
    interface_id: InterfaceId,
    req_eth: &EthernetHeader,
    req_data: &[u8],
) -> Result<(), NetError> {
    let Some(req_arp) = ArpPacket::from_bytes(req_data) else {
        err!(NetError::MalformedPacket);
    };

    // we only support Ethernet+IPv4 ARP packets, so ignore anything else
    if req_arp.htype() != ArpHType::Ethernet || EtherType::from(req_arp.ptype) != EtherType::Ipv4 {
        return Ok(());
    }

    // verify tpa matches our IP address to avoid replying to ARP requests for other hosts
    if let Some(interface) = interface::find_interface_by_id(interface_id) {
        if interface.config.ipv4.map(|ipv4| ipv4.addr) != Some(req_arp.tpa) {
            return Ok(());
        }
    } else {
        err!(NetError::NotConfigured);
    };

    match req_arp.op() {
        ArpOp::Request => {
            try_log!(reply(interface_id, req_eth, req_arp));
        }
        ArpOp::Response => {
            ArpCache::insert(req_arp.spa, req_arp.sha);
            OutgoingQueue::dispatch(req_arp.spa, req_arp.sha);
        }
        ArpOp::Unknown => {}
    }

    Ok(())
}

/// Sends an ARP Reply in response to the given ARP Request.
/// It constructs both the Ethernet and ARP headers for the reply.
fn reply(
    interface_id: InterfaceId,
    req_eth: &EthernetHeader,
    req_arp: &ArpPacket,
) -> Result<(), NetError> {
    let mut packet = [0u8; EthernetHeader::SIZE + ArpPacket::SIZE];
    let mut offset = 0;

    let Some(interface) = interface::find_interface_by_id(interface_id) else {
        err!(NetError::NotConfigured);
    };

    let mac = interface.config.mac;
    let Some(ipv4) = interface.config.ipv4 else {
        err!(NetError::NotConfigured);
    };

    EthernetHeader::new_reply(req_eth, mac).append_to(&mut packet, &mut offset);
    ArpPacket::new_reply(req_arp, mac, ipv4.addr).append_to(&mut packet, &mut offset);

    log!(interface.device.transmit(&packet))
}

/// Sends an ARP Request for the given destination IP address.
/// It constructs both the Ethernet and ARP headers for the request.
///
/// The reply will be handled asynchronously by `handle_arp` and the result will be stored in the
/// ARP cache for future lookups.
pub fn request(interface_id: InterfaceId, dest_ip: Ipv4Addr) -> Result<(), NetError> {
    let mut packet = [0u8; EthernetHeader::SIZE + ArpPacket::SIZE];
    let mut offset = 0;

    let Some(interface) = interface::find_interface_by_id(interface_id) else {
        err!(NetError::NotConfigured);
    };

    let mac = interface.config.mac;
    let Some(ipv4) = interface.config.ipv4 else {
        err!(NetError::NotConfigured);
    };

    EthernetHeader::new(MacAddr::BROADCAST, mac, EtherType::Arp)
        .append_to(&mut packet, &mut offset);
    ArpPacket::new_request(dest_ip, mac, ipv4.addr).append_to(&mut packet, &mut offset);

    log!(interface.device.transmit(&packet))
}
