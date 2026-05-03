use core::slice;

use alloc::boxed::Box;
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

    fn is_loopback(&self) -> bool {
        *self == Ipv4Addr([127, 0, 0, 1])
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

/// Trait for network interfaces, which can transmit packets and provide their MAC and IP addresses.
pub trait NetInterface: Send + Sync {
    fn name(&self) -> &'static str;
    fn mac(&self) -> MacAddr;
    fn ip(&self) -> Option<Ipv4Addr>;
    fn set_ip(&self, ip: Ipv4Addr);
    fn transmit(&self, packet: &[u8]) -> Result<(), NetError>;
    fn needs_arp(&self) -> bool {
        true
    }
    fn needs_dhcp(&self) -> bool {
        true
    }
}

static INTERFACES: SpinLock<Vec<Arc<dyn NetInterface>>> =
    SpinLock::new(Vec::new(), "net_interfaces");

/// Registers a network interface with the network stack, making it available for sending and
/// receiving packets.
///
/// If the interface requires DHCP, this will also trigger a DHCP request to obtain an IP address
/// for the interface. The interface will be added to the list of available interfaces regardless of
/// whether the DHCP request succeeds or fails, since it may still be able to send packets with
/// a manually configured IP address or just use the MAC address for ARP requests.
pub fn register_interface(interface: Arc<dyn NetInterface>) {
    INTERFACES.lock().push(interface.clone());
    if interface.needs_dhcp() {
        let _ = log!(dhcp::request_dhcp(interface));
    }
}

/// Finds a network interface by name.
pub fn find_interface_by_name(name: &str) -> Option<Arc<dyn NetInterface>> {
    let ifs = INTERFACES.lock();
    ifs.iter().find(|i| i.name() == name).cloned()
}

/// Finds a network interface by MAC address.
pub fn find_interface_by_mac(addr: MacAddr) -> Option<Arc<dyn NetInterface>> {
    let ifs = INTERFACES.lock();
    ifs.iter().find(|i| i.mac() == addr).cloned()
}

/// Finds a network interface by IP address.
pub fn find_interface_by_ip(addr: Ipv4Addr) -> Option<Arc<dyn NetInterface>> {
    let ifs = INTERFACES.lock();
    ifs.iter().find(|i| i.ip() == Some(addr)).cloned()
}

/// Finds the appropriate network interface to use for sending a packet to the given destination IP
/// address, based on a routing table.
pub fn route_by_ip(dest_ip: Ipv4Addr) -> Result<Arc<dyn NetInterface>, NetError> {
    if dest_ip.is_loopback() || find_interface_by_ip(dest_ip).is_some() {
        find_interface_by_name("lo")
    } else {
        // TODO: currently only route to e1000
        find_interface_by_name("e1000")
    }
    .ok_or(NetError::NotConfigured)
}

/// Entry in the outgoing packet queue, containing the destination IP address, the interface to send
/// on, and the packet data.
#[derive(Clone)]
struct OutgoingQueueEntry {
    dest_ip: Ipv4Addr,
    interface: Arc<dyn NetInterface>,
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
        interface: Arc<dyn NetInterface>,
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

/// Queue of incoming packets that have been received but not yet processed by the network thread.
/// This queue is shared between all network interfaces.
#[derive(Debug, Clone)]
struct IncomingQueue {
    slots: [Option<Box<[u8]>>; INCOMING_QUEUE_SIZE],
}

static INCOMING_QUEUE: SpinLock<IncomingQueue> = SpinLock::new(
    IncomingQueue {
        slots: [const { None }; INCOMING_QUEUE_SIZE],
    },
    "net_in",
);

impl IncomingQueue {
    /// Enqueues a received packet to be processed by the network thread.
    fn enqueue(packet: Box<[u8]>) -> Result<(), NetError> {
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
    fn dequeue(&mut self) -> Option<Box<[u8]>> {
        self.slots.iter_mut().find_map(|slot| slot.take())
    }
}

/// Enqueues an incoming packet to be processed by the network thread.
/// It wakes up the thread regardless of the enqueue result, since if the queue is full, the thread
/// should be awoken to drain it.
pub fn enqueue_incoming(packet: Box<[u8]>) -> Result<(), NetError> {
    let result = log!(IncomingQueue::enqueue(packet));
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

    if log!(interface.transmit(packet)).is_err() {
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

    let src_mac = interface.mac();
    let Some(src_ip) = interface.ip() else {
        err!(NetError::NotConfigured);
    };

    let payload_len = segments.iter().fold(0, |acc, &s| acc + s.len());

    let eth = EthernetHeader::new(MacAddr::UNSPECIFIED, src_mac, EtherType::Ipv4);
    let ipv4 = Ipv4Header::new(src_ip, dest_ip, proto, payload_len).add_checksum();

    let mut packet = Vec::with_capacity(EthernetHeader::SIZE + ipv4.len() as usize);

    packet.extend_from_slice(eth.as_bytes());
    packet.extend_from_slice(ipv4.as_bytes());

    for segment in segments {
        packet.extend_from_slice(segment);
    }

    let mut packet = packet.into_boxed_slice();

    if !interface.needs_arp() {
        // packet does not require address resolution, send it.
        log!(interface.transmit(&packet))
    } else if let Some(dest_mac) = ArpCache::lookup(dest_ip) {
        // packet requires MAC address but we have it cached, fill it and send it.
        packet[..6].copy_from_slice(&dest_mac.0);
        log!(interface.transmit(&packet))
    } else {
        // packet requires MAC address and we don't have it, queue the packet and send an ARP
        // request. once the reply is received, the packet will be sent.
        try_log!(OutgoingQueue::enqueue(dest_ip, interface.clone(), packet));
        log!(arp::request(interface.clone(), dest_ip))
    }
}

/// Receives a packet, parses the Ethernet header, and dispatches to the appropriate handler based
/// on the EtherType.
fn receive(packet: Box<[u8]>) -> Result<(), NetError> {
    let Some((eth, data)) = EthernetHeader::from_bytes_with_rest(&packet) else {
        err!(NetError::MalformedPacket);
    };

    match eth.ether_type() {
        EtherType::Arp => log!(arp::handle_arp(eth, data)),
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
            let _ = log!(receive(packet));
        } else {
            queue = proc::sleep(Channel::Network, queue);
        }
    }
}

/// Initializes the network stack by spawning a kernel thread to handle incoming packets.
pub fn init() {
    loopback::init();
    proc::spawn_kernel_thread(net_thread, "net");
    proc::spawn_kernel_thread(dhcp::dhcp_thread, "dhcp");
}
