use core::slice;

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::e1000;
use crate::net::arp::ArpCache;
use crate::net::eth::{EtherType, EthernetHeader};
use crate::net::ipv4::{Ipv4Header, Ipv4Proto};
use crate::proc;
use crate::spinlock::SpinLock;

pub(crate) mod arp;
pub(crate) mod eth;
pub(crate) mod icmp;
pub(crate) mod ipv4;
pub(crate) mod udp;

const OUTGOING_QUEUE_SIZE: usize = 16;

static NET_CONFIG: SpinLock<NetConfig> = SpinLock::new(
    NetConfig {
        // default MAC address for QEMU e1000
        mac: MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
        // TODO: use dhcp
        ip: Some(Ipv4Addr([192, 168, 10, 2])),
    },
    "net_config",
);

/// Network configuration, including our MAC address and optionally our IP address (if configured).
#[derive(Debug)]
struct NetConfig {
    mac: MacAddr,
    ip: Option<Ipv4Addr>,
}

/// Trait for integer types that can be converted to/from network byte order (big-endian).
pub trait NetworkInt: Sized + Copy {
    fn to_be(self) -> Self;
    fn from_be(self) -> Self;
}

impl NetworkInt for u16 {
    fn to_be(self) -> Self {
        u16::to_be(self)
    }

    fn from_be(self) -> Self {
        u16::from_be(self)
    }
}

impl NetworkInt for u32 {
    fn to_be(self) -> Self {
        u32::to_be(self)
    }

    fn from_be(self) -> Self {
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
        self.0.from_be()
    }
}

/// MAC Address
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct MacAddr([u8; 6]);

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
pub struct Ipv4Addr([u8; 4]);

impl core::fmt::Display for Ipv4Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}.{}.{}.{}", self.0[0], self.0[1], self.0[2], self.0[3])
    }
}

#[derive(Debug, Clone)]
struct OutgoingQueueEntry {
    dest_ip: Ipv4Addr,
    packet: Box<[u8]>,
}

/// Queue of outgoing packets that needs ARP resolution before they can be transmitted.
///
/// Destination MAC address is filled in when the ARP reply is received, and then the packet is
/// transmitted.
#[derive(Debug)]
struct OutgoingQueue {
    slot: [Option<OutgoingQueueEntry>; OUTGOING_QUEUE_SIZE],
}

static OUTGOING_QUEUE: SpinLock<OutgoingQueue> = SpinLock::new(
    OutgoingQueue {
        slot: [const { None }; OUTGOING_QUEUE_SIZE],
    },
    "net_out",
);

impl OutgoingQueue {
    /// Enqueues a packet for the given destination IP address, to be transmitted once the MAC
    /// address is resolved via ARP.
    ///
    /// Returns an error if the queue is full.
    fn enqueue(dest_ip: Ipv4Addr, packet: Box<[u8]>) -> Result<(), ()> {
        let mut queue = OUTGOING_QUEUE.lock();

        if let Some(slot) = queue.slot.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(OutgoingQueueEntry { dest_ip, packet });
            Ok(())
        } else {
            // TODO: use NetError maybe?
            Err(())
        }
    }

    /// Dispatches any queued packets for the given IP address by filling in the destination MAC
    /// address and transmitting them.
    pub fn dispatch(ip: Ipv4Addr, mac: MacAddr) {
        let mut queue = OUTGOING_QUEUE.lock();

        for slot in queue.slot.iter_mut() {
            if let Some(entry) = slot.as_mut().filter(|s| s.dest_ip == ip) {
                // patch destination mac address
                // always the first 6 bytes of an ethernet frame
                entry.packet[..6].copy_from_slice(&mac.0);
                let _ = log!(e1000::transmit(&entry.packet));

                *slot = None;
            }
        }
    }
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

/// Computes the one's-complement Internet checksum used by IP and ICMP (RFC 1071).
///
/// The algorithm sums all 16-bit big-endian words, folds any carry bits back into the low 16 bits,
/// then returns the one's complement.
///
/// Before calling this, zero out the checksum field in the header so it is not included.
fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;

    let mut chunks = data.chunks_exact(2);
    for chunk in &mut chunks {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }

    // If there is a trailing odd byte, pad it with a zero on the right.
    if let Some(&byte) = chunks.remainder().first() {
        sum += (byte as u32) << 8;
    }

    // Fold the 32-bit accumulator into 16 bits.
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }

    !(sum as u16)
}

fn transmit_raw(packet: &[u8]) {
    let _ = log!(e1000::transmit(packet));
}

/// Transmits a packet by passing it to the e1000 driver, and logs an error if transmission fails.
///
/// Currently only support IPv4 packets.
fn transmit(dest_ip: Ipv4Addr, proto: Ipv4Proto, payload: &[u8]) {
    let (src_mac, src_ip) = {
        let guard = NET_CONFIG.lock();
        let Some(ip) = guard.ip else { return };
        (guard.mac, ip)
    };

    let eth = EthernetHeader::new(MacAddr::UNSPECIFIED, src_mac, EtherType::Ipv4);
    let ipv4 = Ipv4Header::new(src_ip, dest_ip, proto, payload.len()).add_checksum();

    let mut packet = Vec::with_capacity(EthernetHeader::SIZE + ipv4.len() as usize);

    packet.extend_from_slice(eth.as_bytes());
    packet.extend_from_slice(ipv4.as_bytes());
    packet.extend_from_slice(payload);

    let mut packet = packet.into_boxed_slice();

    // if we have the destination MAC address cached, fill it in and transmit immediately, otherwise
    // queue the packet and send an ARP request to resolve the MAC address, which will trigger
    // transmission when the reply is received.
    if let Some(dest_mac) = ArpCache::lookup(dest_ip) {
        packet[..6].copy_from_slice(&dest_mac.0);
        transmit_raw(&packet);
    } else {
        let _ = log!(OutgoingQueue::enqueue(dest_ip, packet));
        arp::request(dest_ip);
    }
}

/// Receives a packet, parses the Ethernet header, and dispatches to the appropriate handler based
/// on the EtherType.
fn receive(packet: Box<[u8]>) {
    let Some((eth, data)) = EthernetHeader::from_bytes_with_rest(&packet) else {
        return;
    };

    match eth.ether_type() {
        EtherType::Arp => arp::handle_arp(eth, data),
        EtherType::Ipv4 => ipv4::handle_ipv4(data),
        EtherType::Unknown => {}
    }
}

/// Kernel thread that continuously receives packets from the e1000 driver and processes them.
fn net_thread() {
    loop {
        receive(e1000::receive());
    }
}

/// Initializes the network stack by spawning a kernel thread to handle incoming packets.
pub fn init() {
    proc::spawn_kernel_thread(net_thread, "net");
}
