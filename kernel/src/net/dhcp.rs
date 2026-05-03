use alloc::sync::Arc;

use crate::net::eth::{EtherType, EthernetHeader};
use crate::net::ipv4::{Ipv4Header, Ipv4Proto};
use crate::net::udp::{SocketTable, UdpHeader};
use crate::net::{Be, Ipv4Addr, MacAddr, NetError, NetInterface, NetworkHeader};
use crate::rng;
use crate::spinlock::SpinLock;

const DHCP_QUEUE_SIZE: usize = 16;

/// Vendor Information "Magic Cookie"
/// https://www.rfc-editor.org/rfc/rfc1497
const MAGIC_COOKIE: [u8; 4] = [0x63, 0x82, 0x53, 0x63];

/// DHCP's BOOTP header structure as defined in RFC 2131
/// https://www.rfc-editor.org/rfc/rfc2131
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct BootpHeader {
    /// Message op code (1 = BOOTREQUEST, 2 = BOOTREPLY)
    op: u8,
    /// Hardware address type (1 = Ethernet)
    htype: u8,
    /// Hardware address length (6 for Ethernet)
    hlen: u8,
    /// Client sets to zero, optionally used by relay agents when booting via a relay agent
    hops: u8,
    /// Transaction ID, a random number chosen by the client
    xid: Be<u32>,
    /// Seconds elapsed since client began address acquisition or renewal process
    secs: Be<u16>,
    /// Flags
    flags: Be<u16>,
    /// Client IP address (0.0.0.0 on first request, client fills in if renewing)
    ciaddr: Ipv4Addr,
    /// 'your' IP address (server fills this with the offered address)
    yiaddr: Ipv4Addr,
    /// Next server IP address
    siaddr: Ipv4Addr,
    /// Relay agent IP address
    giaddr: Ipv4Addr,
    /// Client hardware address
    chaddr: [u8; 16],
    /// Optional server host name, null-terminated string
    sname: [u8; 64],
    /// Optional boot filename, null-terminated string
    file: [u8; 128],
}

impl BootpHeader {
    const SIZE: usize = size_of::<Self>();

    /// Creates a new BOOTREQUEST header with default values, to be filled in by the caller.
    fn new_request() -> Self {
        Self {
            op: 1, // op=1 for BOOTREQUEST
            htype: 1,
            hlen: 6,
            hops: 0,
            xid: Be::new(0), // to be filled with a random transaction ID
            secs: Be::new(0),
            flags: Be::new(0),
            ciaddr: Ipv4Addr::UNSPECIFIED,
            yiaddr: Ipv4Addr::UNSPECIFIED,
            siaddr: Ipv4Addr::UNSPECIFIED,
            giaddr: Ipv4Addr::UNSPECIFIED,
            chaddr: [0; 16], // to be filled with client's MAC address
            sname: [0; 64],
            file: [0; 128],
        }
    }
}

unsafe impl NetworkHeader for BootpHeader {}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DhcpOptionType {
    RequestedIpAddress = 50,
    MessageType = 53,
    ServerIdentifier = 54,
    Unknown = u8::MAX,
}

impl From<u8> for DhcpOptionType {
    fn from(value: u8) -> Self {
        match value {
            50 => Self::RequestedIpAddress,
            53 => Self::MessageType,
            54 => Self::ServerIdentifier,
            _ => Self::Unknown,
        }
    }
}

impl From<DhcpOptionType> for u8 {
    fn from(value: DhcpOptionType) -> Self {
        value as u8
    }
}

/// Represents a single DHCP option, which consists of an ID and its associated data.
#[derive(Debug)]
struct DhcpOption<'a> {
    id: DhcpOptionType,
    data: &'a [u8],
}

impl<'a> DhcpOption<'a> {
    fn new(id: DhcpOptionType, data: &'a [u8]) -> Self {
        Self { id, data }
    }

    fn byte_len(&self) -> usize {
        // 1 byte for ID + 1 byte for length + data length
        2 + self.data.len()
    }

    /// Appends the option to the given buffer at the specified offset, updating the offset
    /// accordingly.
    ///
    /// This mirrors `NetworkHeader`'s `append_to()` method since `#[repr(C, packed)]` is not possible with
    /// referenced data.
    fn append_to(&self, buf: &mut [u8], offset: &mut usize) {
        buf[*offset] = self.id.into();
        *offset += 1;
        buf[*offset] = self.data.len() as u8;
        *offset += 1;
        buf[*offset..*offset + self.data.len()].copy_from_slice(self.data);
        *offset += self.data.len();
    }
}

/// Reader type for iterating over DHCP options in a received packet, which are encoded as TLV
/// (Type-Length-Value) fields until the End Option (0xFF) is encountered.
#[derive(Debug, Clone)]
struct DhcpOptionIter<'a>(&'a [u8]);

impl<'a> Iterator for DhcpOptionIter<'a> {
    type Item = DhcpOption<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() {
            return None;
        }

        if self.0[0] == 0xFF {
            // End Option
            self.0 = &self.0[1..];
            return None;
        }

        if self.0.len() < 2 {
            // Malformed option
            return None;
        }

        let id = self.0[0].into();
        let length = self.0[1];
        if self.0.len() < 2 + length as usize {
            // Malformed option
            return None;
        }

        let data = &self.0[2..2 + length as usize];

        self.0 = &self.0[2 + length as usize..];

        Some(DhcpOption { id, data })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DhcpState {
    PendingDiscover,
    WaitingForOffer {
        xid: u32,
    },
    WaitingForAck {
        xid: u32,
        offered_ip: Ipv4Addr,
        server_ip: Ipv4Addr,
    },
}

#[derive(Clone)]
struct DhcpQueueEntry {
    interface: Arc<dyn NetInterface>,
    state: DhcpState,
}

struct DhcpQueue {
    entries: [Option<DhcpQueueEntry>; DHCP_QUEUE_SIZE],
}

static DHCP_QUEUE: SpinLock<DhcpQueue> = SpinLock::new(
    DhcpQueue {
        entries: [const { None }; DHCP_QUEUE_SIZE],
    },
    "dhcp_queue",
);

impl DhcpQueue {
    /// Enqueues a new DHCP request for the given network interface, returning an error if the queue
    /// is full.
    fn enqueue(interface: Arc<dyn NetInterface>) -> Result<(), NetError> {
        let mut queue = DHCP_QUEUE.lock();

        if let Some(entry) = queue.entries.iter_mut().find(|entry| entry.is_none()) {
            *entry = Some(DhcpQueueEntry {
                interface,
                state: DhcpState::PendingDiscover,
            });
            Ok(())
        } else {
            err!(NetError::QueueFull)
        }
    }

    /// Returns the first `PendingDiscover` entry's index and interface.
    fn next_pending_discover() -> Option<(usize, Arc<dyn NetInterface>)> {
        let queue = DHCP_QUEUE.lock();

        queue.entries.iter().enumerate().find_map(|(i, entry)| {
            let e = entry.as_ref()?;
            matches!(e.state, DhcpState::PendingDiscover).then(|| (i, e.interface.clone()))
        })
    }

    /// Marks the entry at the given index as having sent a Discover message with the specified
    /// transaction ID `xid`, transitioning it to the `WaitingForOffer` state.
    fn mark_discover_sent(index: usize, xid: u32) {
        assert!(index < DHCP_QUEUE_SIZE);

        let mut queue = DHCP_QUEUE.lock();

        if let Some(entry) = &mut queue.entries[index] {
            entry.state = DhcpState::WaitingForOffer { xid };
        }
    }

    /// Handles a received DHCP Offer by finding the corresponding entry in the queue that is
    /// waiting for an offer with the matching transaction ID `xid`, and transitioning it to the
    /// `WaitingForAck` state with the offered IP address and server IP from the offer.
    ///
    /// It then sends a DHCP Request to the server to accept the offer.
    fn on_offer(xid: u32, server_ip: Ipv4Addr, offered_ip: Ipv4Addr) {
        let mut queue = DHCP_QUEUE.lock();

        if let Some(entry) = queue.entries.iter_mut().find_map(|entry| {
            let e = entry.as_mut()?;
            matches!(e.state, DhcpState::WaitingForOffer { xid: x } if x == xid).then_some(e)
        }) {
            entry.state = DhcpState::WaitingForAck {
                xid,
                offered_ip,
                server_ip,
            };

            request(entry.interface.clone(), xid, server_ip, offered_ip);
        }
    }

    /// Handles a received DHCP Ack by finding the corresponding entry in the queue that is waiting
    /// for an ack with the matching transaction ID `xid`, removing it from the queue, and returning
    /// it to the caller.
    fn on_ack(xid: u32) {
        let mut queue = DHCP_QUEUE.lock();

        let entry = queue.entries.iter_mut().find(|entry| {
            matches!(
                entry.as_ref(),
                Some(e) if matches!(e.state, DhcpState::WaitingForAck { xid: x, .. } if x == xid)
            )
        });

        if let Some(entry) = entry {
            let ack = entry.take().unwrap();

            let offered_ip = match ack.state {
                // TODO: all other options like subnet mask, gateway, DNS, lease time, etc.
                DhcpState::WaitingForAck { offered_ip, .. } => offered_ip,
                _ => unreachable!(),
            };

            ack.interface.set_ip(offered_ip);
        }
    }
}

/// Transmits a DHCP packet with the given transaction ID and options on the specified interface.
///
/// This function constructs the entire Ethernet + IPv4 + UDP + BOOTP + DHCP packet from scratch
/// since we cannot use UDP's `SocketTable::send()` without an IP address, and DHCP requires
/// specific handling of broadcast packets and options.
fn transmit_dhcp(
    interface: Arc<dyn NetInterface>,
    xid: u32,
    options: &[DhcpOption],
) -> Result<(), NetError> {
    // calculate the total size of the DHCP options (+1 for the END option)
    let options_size = options.iter().map(DhcpOption::byte_len).sum::<usize>() + 1;
    let dhcp_size = BootpHeader::SIZE + MAGIC_COOKIE.len() + options_size;

    // arbitrary limit to static buffer size, can be increased if needed
    if options_size > 256 {
        err!(NetError::MalformedPacket);
    }

    // cannot use UDP's SocketTable::send() since it requires the IP address to be available.
    // create the whole network packet from scratch
    let mut packet = [0u8; EthernetHeader::SIZE
        + Ipv4Header::SIZE
        + UdpHeader::SIZE
        + BootpHeader::SIZE
        + MAGIC_COOKIE.len()
        + 256 /* extra space for options */];
    let mut offset = 0;

    let mac = interface.mac();

    EthernetHeader::new(MacAddr::BROADCAST, mac, EtherType::Ipv4)
        .append_to(&mut packet, &mut offset);

    Ipv4Header::new(
        Ipv4Addr::UNSPECIFIED,
        Ipv4Addr::BROADCAST,
        Ipv4Proto::Udp,
        UdpHeader::SIZE + dhcp_size,
    )
    .add_checksum()
    .append_to(&mut packet, &mut offset);

    // always send from client (68) to server (67)
    // UDP offset is ignored for DHCP, no need to calculate
    UdpHeader::new(68, 67, dhcp_size).append_to(&mut packet, &mut offset);

    let mut header = BootpHeader::new_request();
    // copy our MAC address into the header
    header.chaddr[..6].copy_from_slice(&mac.0);
    // set the broadcast flag since we don't have an IP address yet
    header.flags = Be::new(0x8000u16);
    // set the transaction ID to match between Discover and Request
    header.xid = Be::new(xid);

    header.append_to(&mut packet, &mut offset);

    packet[offset..offset + MAGIC_COOKIE.len()].copy_from_slice(&MAGIC_COOKIE);
    offset += MAGIC_COOKIE.len();

    for option in options {
        option.append_to(&mut packet, &mut offset);
    }

    packet[offset] = 0xFF; // End Option
    offset += 1;

    log!(interface.transmit(&packet[..offset]))
}

/// Sends a DHCP Discover to find available DHCP servers and obtain an IP address lease.
///
/// This is the first step in the DHCP state machine, and is typically called when an interface is
/// initialized without an IP address.
///
/// The client generates a random transaction ID to match the Discover with subsequent Offer and
/// Request messages from the server.
fn discover(index: usize, interface: Arc<dyn NetInterface>) {
    let options = [
        // DHCP Discover
        DhcpOption::new(DhcpOptionType::MessageType, &[1]),
    ];

    let xid = rng::rand_u32();

    if log!(transmit_dhcp(interface, xid, &options)).is_ok() {
        DhcpQueue::mark_discover_sent(index, xid);
    }
}

/// Sends a DHCP Request in response to a received DHCP Offer.
fn request(interface: Arc<dyn NetInterface>, xid: u32, server_ip: Ipv4Addr, offered_ip: Ipv4Addr) {
    let options = [
        // DHCP Request
        DhcpOption::new(DhcpOptionType::MessageType, &[3]),
        // Request the same IP address that was offered to us
        DhcpOption::new(DhcpOptionType::RequestedIpAddress, &offered_ip.0),
        // Include the Server Identifier to specify which server we're accepting the offer from
        DhcpOption::new(DhcpOptionType::ServerIdentifier, &server_ip.0),
    ];

    let _ = log!(transmit_dhcp(interface, xid, &options));
}

/// Public API for requesting an IP address lease via DHCP on the given network interface.
/// This enqueues a DHCP Discover request to be processed by the DHCP client thread, which will
/// handle the entire DHCP state machine and update the interface's IP address using
/// `NetInterface::set_ip()` when a lease is obtained.
pub fn request_dhcp(interface: Arc<dyn NetInterface>) -> Result<(), NetError> {
    DhcpQueue::enqueue(interface)
}

/// Main loop for the DHCP client thread, which listens for incoming DHCP packets and handles them
/// according to the DHCP state machine.
pub fn dhcp_thread() {
    let socket = SocketTable::open(68).expect("failed to open DHCP client port");

    loop {
        // process pending discover request
        while let Some((index, interface)) = DhcpQueue::next_pending_discover() {
            discover(index, interface);
        }

        // receive incoming DHCP packets
        if let Ok(Some((_ip, _port, packet))) = SocketTable::try_receive(socket) {
            let Some((header, options)) = BootpHeader::from_bytes_with_rest(&packet) else {
                continue;
            };

            if header.op != 2 || options.len() < 4 || options[..4] != MAGIC_COOKIE {
                continue;
            }

            let mut message_type: Option<u8> = None;
            let mut server_id: Option<Ipv4Addr> = None;

            // DhcpOptionIter is responsible from verifying the data length
            for option in DhcpOptionIter(&options[4..]) {
                match option.id {
                    DhcpOptionType::MessageType => message_type = Some(option.data[0]),
                    DhcpOptionType::ServerIdentifier => {
                        server_id = Some(Ipv4Addr(option.data.try_into().unwrap()));
                    }
                    _ => {}
                }
            }

            match message_type {
                // DHCP Offer
                Some(2) => {
                    if let Some(server) = server_id {
                        DhcpQueue::on_offer(header.xid.get(), server, Ipv4Addr(header.yiaddr.0));
                    }
                }

                // DHCP Ack
                Some(5) => {
                    DhcpQueue::on_ack(header.xid.get());
                }

                _ => {}
            }
        }
    }
}
