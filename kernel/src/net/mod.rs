use core::slice;
use core::sync::atomic::{AtomicU16, Ordering};

use alloc::boxed::Box;
use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::net::arp::ArpCache;
use crate::net::eth::{EtherType, EthernetHeader};
use crate::net::ipv4::{Ipv4Header, Ipv4Proto};
use crate::proc::{self, Channel};
use crate::spinlock::SpinLock;

pub(crate) mod arp;
pub(crate) mod dhcp;
pub(crate) mod eth;
pub(crate) mod icmp;
pub(crate) mod ipv4;
pub(crate) mod loopback;
pub(crate) mod udp;

const OUTGOING_QUEUE_SIZE: usize = 16;
const INCOMING_QUEUE_SIZE: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum NetError {
    NotConfigured = 1,
    QueueFull,
    TableFull,
    OutOfSocket,
    PortInUse,
    BadSocket,
    InvalidAddress,
    MalformedPacket,
    TransmitFailed,
    Interrupted,
    RouteNotFound,
    PacketTooLarge,
    ResourceUnavailable,
    InterfaceNotFound,
}

impl core::fmt::Display for NetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            NetError::NotConfigured => write!(f, "network is not configured"),
            NetError::QueueFull => write!(f, "outgoing packet queue is full"),
            NetError::TableFull => write!(f, "table is full"),
            NetError::OutOfSocket => write!(f, "out of socket"),
            NetError::PortInUse => write!(f, "port is already in use"),
            NetError::BadSocket => write!(f, "bad socket"),
            NetError::InvalidAddress => write!(f, "invalid address"),
            NetError::MalformedPacket => write!(f, "malformed packet"),
            NetError::TransmitFailed => write!(f, "packet transmission failed"),
            NetError::Interrupted => write!(f, "operation interrupted"),
            NetError::RouteNotFound => write!(f, "route not found"),
            NetError::PacketTooLarge => write!(f, "packet is too large"),
            NetError::ResourceUnavailable => write!(f, "resource unavailable"),
            NetError::InterfaceNotFound => write!(f, "interface not found"),
        }
    }
}

/// Trait for integer types that can be converted to/from network byte order (big-endian).
pub trait NetworkInt: Sized + Copy {
    fn to_be(self) -> Self;
    fn into_be(self) -> Self;
}

impl NetworkInt for u16 {
    fn to_be(self) -> Self {
        u16::to_be(self)
    }

    fn into_be(self) -> Self {
        u16::from_be(self)
    }
}

impl NetworkInt for u32 {
    fn to_be(self) -> Self {
        u32::to_be(self)
    }

    fn into_be(self) -> Self {
        u32::from_be(self)
    }
}

/// Wrapper type for network byte order (big-endian) integers, providing methods to convert to/from
/// host byte order.
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct Be<T: Copy>(T);

impl<T: NetworkInt> Be<T> {
    fn new(value: T) -> Self {
        Self(value.to_be())
    }

    fn get(self) -> T {
        self.0.into_be()
    }
}

/// MAC Address
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    const UNSPECIFIED: Self = Self([0x00; 6]);
    const BROADCAST: Self = Self([0xff; 6]);
}

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
pub struct Ipv4Addr(pub [u8; 4]);

impl Ipv4Addr {
    pub const UNSPECIFIED: Self = Self([0; 4]);
    pub const BROADCAST: Self = Self([255; 4]);
    pub const LOOPBACK: Self = Self([127, 0, 0, 1]);

    /// Returns the subnet mask corresponding to the given prefix length.
    pub fn prefix_len_to_mask(prefix_len: u8) -> Self {
        if prefix_len == 0 {
            Self([0; 4])
        } else {
            let mask = u32::MAX.checked_shl(32 - prefix_len as u32).unwrap_or(0);
            Self(mask.to_be_bytes())
        }
    }

    pub fn mask_to_prefix_len(mask: Self) -> Option<u8> {
        let mask = u32::from_be_bytes(mask.0);

        // valid mask must be contiguous 1s then 0s.
        let prefix_len = mask.leading_ones() as u8;
        let expected = if prefix_len == 0 {
            0
        } else {
            u32::MAX << (32 - prefix_len as u32)
        };

        (mask == expected).then_some(prefix_len)
    }
}

impl core::ops::BitAnd for Ipv4Addr {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        let a = u32::from_be_bytes(self.0);
        let b = u32::from_be_bytes(rhs.0);
        Self((a & b).to_be_bytes())
    }
}

impl core::str::FromStr for Ipv4Addr {
    type Err = NetError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut octets = [0u8; 4];
        let mut i = 0;

        for octet in s.split('.') {
            if i >= 4 {
                err!(NetError::InvalidAddress);
            }

            octets[i] = match octet.parse() {
                Ok(n) => n,
                Err(_) => err!(NetError::InvalidAddress),
            };
            i += 1;
        }

        if i != 4 {
            err!(NetError::InvalidAddress);
        }

        Ok(Ipv4Addr(octets))
    }
}

impl core::fmt::Display for Ipv4Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

/// Trait for network interface devices
pub trait NetDevice: Send + Sync {
    fn transmit(&self, packet: &[u8]) -> Result<(), NetError>;
    fn needs_arp(&self) -> bool {
        true
    }
    fn needs_dhcp(&self) -> bool {
        true
    }
}

/// Identifier for a network interface, used in routing table entries to specify which interface to
/// send packets on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceId(pub u16);

impl InterfaceId {
    pub fn next_interface_id() -> Self {
        static ID_COUNTER: AtomicU16 = AtomicU16::new(0);
        Self(ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// IPv4 configuration for a network interface.
#[derive(Debug, Clone, Copy)]
pub struct Ipv4Config {
    /// IPv4 address
    pub addr: Ipv4Addr,
    /// Number of leading bits in the subnet mask.
    pub prefix_len: u8,
}

/// Configuration for a network interface.
#[derive(Debug, Clone)]
pub struct InterfaceConfig {
    /// Name
    pub name: &'static str,
    /// MAC address
    pub mac: MacAddr,
    /// IPv4 configuration, if configured.
    /// If `None`, the interface is not configured and cannot send or receive IP packets.
    pub ipv4: Option<Ipv4Config>,
    /// Whether the interface is functional or not.
    pub is_up: bool,
}

/// Interface information for routing and packet transmission.
#[derive(Clone)]
pub struct InterfaceEntry {
    /// Identifier
    pub id: InterfaceId,
    /// Configuration used for routing.
    pub config: InterfaceConfig,
    /// Reference to the underlying network device, used for sending and receiving packets.
    pub device: Arc<dyn NetDevice>,
}

/// Owner of a route entry, used for tracking which component added the route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteOwner {
    /// Route added by an interface's address and prefix.
    Interface,
    /// Route added by the DHCP client when it receives an IP address lease.
    Dhcp,
    /// Route added by a static configuration.
    Static,
}

/// Routing information for a destination subnet.
#[derive(Debug, Clone)]
pub struct RouteEntry {
    /// Destination IPv4 address for this route.
    pub dest: Ipv4Addr,
    /// Number of leading bits in the subnet mask for this route.
    pub prefix_len: u8,
    /// Next hop IP address for this route. If `None`, the destination is directly reachable on the
    /// link layer.
    pub gateway: Option<Ipv4Addr>,
    /// Interface to send packets matching this route on.
    pub interface_id: InterfaceId,
    /// Routing metric for this route, used to determine the best route when multiple routes match a
    /// destination. Lower values indicate more preferred routes.
    pub metric: u32,
    /// Owner of this route, used for tracking which component added this route and allowing it to
    /// remove the route later if needed.
    pub owner: RouteOwner,
}

/// Registry of network interfaces and routing information.
pub struct NetRegistry {
    pub interfaces: SpinLock<Vec<InterfaceEntry>>,
    pub routes: SpinLock<Vec<RouteEntry>>,
}

static NET_REGISTRY: NetRegistry = NetRegistry {
    interfaces: SpinLock::new(Vec::new(), "net_interfaces"),
    routes: SpinLock::new(Vec::new(), "net_routes"),
};

/// Adds a route to the routing table.
pub fn add_route(
    dest_ip: Ipv4Addr,
    prefix_len: u8,
    gateway: Option<Ipv4Addr>,
    interface_id: InterfaceId,
    metric: u32,
    owner: RouteOwner,
) {
    NET_REGISTRY.routes.lock().push(RouteEntry {
        dest: dest_ip,
        prefix_len,
        gateway,
        interface_id,
        metric,
        owner,
    });

    let l = NET_REGISTRY.routes.lock();
    println!("{:#?}", *l);
}

/// Registers a network interface with the network stack and returns its assigned `InterfaceId.
pub fn register_interface(config: InterfaceConfig, device: Arc<dyn NetDevice>) -> InterfaceId {
    let id = InterfaceId::next_interface_id();

    if let Some(ipv4) = config.ipv4 {
        add_route(
            ipv4.addr & Ipv4Addr::prefix_len_to_mask(ipv4.prefix_len),
            ipv4.prefix_len,
            None,
            id,
            10, // default metric for interface routes
            RouteOwner::Interface,
        );
    };

    let needs_dhcp = device.needs_dhcp();
    let name = config.name;

    NET_REGISTRY
        .interfaces
        .lock()
        .push(InterfaceEntry { id, config, device });

    if needs_dhcp {
        let thread_name = format!("dhcp-{}", name);
        proc::spawn_kernel_thread(move || dhcp::dhcp_thread(id), &thread_name);
    }

    id
}

/// Sets the IPv4 configuration for a network interface, replacing any existing configuration.
///
/// If `ipv4` is `None`, the interface is marked as not configured and cannot send or receive IP
/// packets.
pub fn set_interface_ipv4(id: InterfaceId, ipv4: Option<Ipv4Config>) -> Result<(), NetError> {
    let mut ifs = NET_REGISTRY.interfaces.lock();

    let Some(interface) = ifs.iter_mut().find(|i| i.id == id) else {
        err!(NetError::InterfaceNotFound)
    };

    interface.config.ipv4 = ipv4;

    // drop interfaces lock before add_route
    drop(ifs);

    if let Some(ipv4) = ipv4 {
        add_route(
            ipv4.addr & Ipv4Addr::prefix_len_to_mask(ipv4.prefix_len),
            ipv4.prefix_len,
            None,
            id,
            10, // default metric for interface routes
            RouteOwner::Interface,
        );
    }

    Ok(())
}

/// Finds a network interface by its identifier.
///
/// Returned entry is a clone of the one in the registry. This value could be stale immediately
/// after it's returned.
pub fn find_interface_by_id(id: InterfaceId) -> Option<InterfaceEntry> {
    let ifs = NET_REGISTRY.interfaces.lock();
    ifs.iter().find(|i| i.id == id).cloned()
}

/// Finds the appropriate network interface to use for sending a packet to the given destination IP
/// address, based on the routing table.
pub fn route_by_ip(dest_ip: Ipv4Addr) -> Result<InterfaceEntry, NetError> {
    let interface_id = {
        let routes = NET_REGISTRY.routes.lock();

        let dest = u32::from_be_bytes(dest_ip.0);

        let Some(best_route) = routes
            .iter()
            .filter(|route| {
                let mask = if route.prefix_len == 0 {
                    0
                } else {
                    u32::MAX << (32 - route.prefix_len as u32)
                };

                let route_dest = u32::from_be_bytes(route.dest.0);

                (dest & mask) == (route_dest & mask)
            })
            // prefer longer prefix length, then lower metric
            .max_by_key(|route| (route.prefix_len, core::cmp::Reverse(route.metric)))
        else {
            err!(NetError::RouteNotFound)
        };

        best_route.interface_id
    }; // drop routes lock before find

    Ok(find_interface_by_id(interface_id).expect("interface in route entry should exist"))
}

/// Entry in the outgoing packet queue, containing the destination IP address, the interface to send
/// on, and the packet data.
#[derive(Clone)]
struct OutgoingQueueEntry {
    dest_ip: Ipv4Addr,
    interface: Arc<dyn NetDevice>,
    packet: Box<[u8]>,
}

/// Queue of outgoing packets that needs ARP resolution before they can be transmitted.
///
/// Destination MAC address is filled in when the ARP reply is received, and then the packet is
/// transmitted.
struct OutgoingQueue {
    slots: [Option<OutgoingQueueEntry>; OUTGOING_QUEUE_SIZE],
}

static OUTGOING_QUEUE: SpinLock<OutgoingQueue> = SpinLock::new(
    OutgoingQueue {
        slots: [const { None }; OUTGOING_QUEUE_SIZE],
    },
    "net_out",
);

impl OutgoingQueue {
    /// Enqueues a packet for the given destination IP address, to be transmitted once the MAC
    /// address is resolved via ARP.
    ///
    /// Returns an error if the queue is full.
    fn enqueue(
        dest_ip: Ipv4Addr,
        interface: Arc<dyn NetDevice>,
        packet: Box<[u8]>,
    ) -> Result<(), NetError> {
        let mut queue = OUTGOING_QUEUE.lock();

        if let Some(slot) = queue.slots.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(OutgoingQueueEntry {
                dest_ip,
                interface,
                packet,
            });
            Ok(())
        } else {
            err!(NetError::QueueFull)
        }
    }

    /// Dispatches any queued packets for the given IP address by filling in the destination MAC
    /// address and transmitting them.
    pub fn dispatch(ip: Ipv4Addr, mac: MacAddr) {
        let mut queue = OUTGOING_QUEUE.lock();

        for slot in queue.slots.iter_mut() {
            if let Some(entry) = slot.as_mut().filter(|s| s.dest_ip == ip) {
                // patch destination mac address
                // always the first 6 bytes of an ethernet frame
                entry.packet[..6].copy_from_slice(&mac.0);
                let _ = log!(entry.interface.transmit(&entry.packet));

                *slot = None;
            }
        }
    }
}

/// Entry in the incoming packet queue.
#[derive(Debug, Clone)]
struct IncomingPacket {
    interface_id: InterfaceId,
    data: Box<[u8]>,
}

/// Queue of incoming packets that have been received but not yet processed by the network thread.
/// This queue is shared between all network interfaces.
#[derive(Debug, Clone)]
struct IncomingQueue {
    slots: [Option<IncomingPacket>; INCOMING_QUEUE_SIZE],
}

static INCOMING_QUEUE: SpinLock<IncomingQueue> = SpinLock::new(
    IncomingQueue {
        slots: [const { None }; INCOMING_QUEUE_SIZE],
    },
    "net_in",
);

impl IncomingQueue {
    /// Enqueues a received packet to be processed by the network thread.
    fn enqueue(packet: IncomingPacket) -> Result<(), NetError> {
        let mut queue = INCOMING_QUEUE.lock();

        if let Some(slot) = queue.slots.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(packet);
            Ok(())
        } else {
            err!(NetError::QueueFull)
        }
    }

    /// Dequeues a packet for processing by the network thread, returning `None` if the queue is
    /// empty.
    ///
    /// Unlike other queue methods, caller holds the lock since it will sleep on the channel with
    /// the lock if the queue is empty.
    fn dequeue(&mut self) -> Option<IncomingPacket> {
        self.slots.iter_mut().find_map(|slot| slot.take())
    }
}

/// Enqueues an incoming packet to be processed by the network thread.
/// It wakes up the thread regardless of the enqueue result, since if the queue is full, the thread
/// should be awoken to drain it.
pub fn enqueue_incoming(interface_id: InterfaceId, packet: Box<[u8]>) -> Result<(), NetError> {
    let result = log!(IncomingQueue::enqueue(IncomingPacket {
        interface_id,
        data: packet,
    }));
    proc::wakeup(Channel::Network);
    result
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

/// Computes the one's-complement internet checksum (RFC 1071) over multiple segments.
///
/// The algorithm sums all 16-bit big-endian words, folds any carry bits back into the low 16 bits,
/// then returns the one's complement.
///
/// Before calling this, zero out the checksum field in the header so it is not included.
fn internet_checksum(segments: &[&[u8]]) -> u16 {
    // Use a 32-bit accumulator to prevent overflows.
    let mut sum: u32 = 0;
    let mut carry: Option<u8> = None;

    for segment in segments {
        if segment.is_empty() {
            continue;
        }

        // Account for the carry byte from the previous segment if any.
        let data = if let Some(byte) = carry.take() {
            sum += u16::from_be_bytes([byte, segment[0]]) as u32;
            &segment[1..]
        } else {
            segment
        };

        // Sum up 16-bit big-endian words in this segment.
        let mut chunks = data.chunks_exact(2);
        for chunk in &mut chunks {
            sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
        }

        // If there is a trailing odd byte, save it for the next segment.
        carry = chunks.remainder().first().copied();
    }

    // There was a trailing odd byte after the last segment, pad by 0x00 and add to the sum.
    if let Some(byte) = carry.take() {
        sum += u16::from_be_bytes([byte, 0]) as u32;
    }

    // Fold the 32-bit accumulator into 16 bits.
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }

    // Negate the result.
    !(sum as u16)
}

/// Transmits a ethernet-layer packet by passing it to a corresponding interface according to the
/// routing table, without performing any additional processing.
///
/// This is used for transmitting ARP packets, which do not require IP-layer processing and may not
/// have a valid destination MAC address at the time of transmission, since they are used for
/// address resolution.
fn transmit_raw(dest_ip: Ipv4Addr, packet: &[u8]) -> Result<(), NetError> {
    let interface = try_log!(route_by_ip(dest_ip));

    if log!(interface.device.transmit(packet)).is_err() {
        err!(NetError::TransmitFailed)
    }

    Ok(())
}

/// Transmits a packet by passing it to a corresponding interface according to the routing table.
/// Packets are passed as a list of segments that will be concatenated together.
///
/// Currently only support IPv4 packets.
fn transmit(dest_ip: Ipv4Addr, proto: Ipv4Proto, segments: &[&[u8]]) -> Result<(), NetError> {
    let interface = try_log!(route_by_ip(dest_ip));

    let src_mac = interface.config.mac;
    let Some(src_ipv4) = interface.config.ipv4 else {
        err!(NetError::NotConfigured);
    };

    let payload_len = segments.iter().fold(0, |acc, &s| acc + s.len());

    let eth = EthernetHeader::new(MacAddr::UNSPECIFIED, src_mac, EtherType::Ipv4);
    let ipv4 = Ipv4Header::new(src_ipv4.addr, dest_ip, proto, payload_len).add_checksum();

    let mut packet = Vec::with_capacity(EthernetHeader::SIZE + ipv4.len() as usize);

    packet.extend_from_slice(eth.as_bytes());
    packet.extend_from_slice(ipv4.as_bytes());

    for segment in segments {
        packet.extend_from_slice(segment);
    }

    let mut packet = packet.into_boxed_slice();

    if !interface.device.needs_arp() {
        // packet does not require address resolution, send it.
        log!(interface.device.transmit(&packet))
    } else if let Some(dest_mac) = ArpCache::lookup(dest_ip) {
        // packet requires MAC address but we have it cached, fill it and send it.
        packet[..6].copy_from_slice(&dest_mac.0);
        log!(interface.device.transmit(&packet))
    } else {
        // packet requires MAC address and we don't have it, queue the packet and send an ARP
        // request. once the reply is received, the packet will be sent.
        try_log!(OutgoingQueue::enqueue(
            dest_ip,
            interface.device.clone(),
            packet
        ));
        log!(arp::request(interface.id, dest_ip))
    }
}

/// Receives a packet, parses the Ethernet header, and dispatches to the appropriate handler based
/// on the EtherType.
fn receive(id: InterfaceId, packet: Box<[u8]>) -> Result<(), NetError> {
    let Some((eth, data)) = EthernetHeader::from_bytes_with_rest(&packet) else {
        err!(NetError::MalformedPacket);
    };

    match eth.ether_type() {
        EtherType::Arp => log!(arp::handle_arp(id, eth, data)),
        EtherType::Ipv4 => log!(ipv4::handle_ipv4(data)),
        EtherType::Unknown => Ok(()),
    }
}

/// Kernel thread that continuously receives packets from the incoming queue and processes them by
/// calling `receive()`.
///
/// This thread is woken up whenever a new packet is enqueued to the incoming queue, and it will
/// process all packets in the queue before going back to sleep.
fn net_thread() {
    loop {
        let mut queue = INCOMING_QUEUE.lock();

        if let Some(packet) = queue.dequeue() {
            drop(queue);
            let _ = log!(receive(packet.interface_id, packet.data));
        } else {
            queue = proc::sleep(Channel::Network, queue);
        }
    }
}

/// Initializes the network stack by spawning a kernel thread to handle incoming packets.
pub fn init() {
    loopback::init();
    proc::spawn_kernel_thread(net_thread, "net");

    println!("net  init");
}
