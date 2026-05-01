use alloc::sync::Arc;

use crate::net::eth::{EtherType, EthernetHeader};
use crate::net::ipv4::{Ipv4Header, Ipv4Proto};
use crate::net::udp::{SocketTable, UdpHeader};
use crate::net::{self, Be, Ipv4Addr, MacAddr, NetError, NetInterface, NetworkHeader};
use crate::rng;

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
    pub const SIZE: usize = size_of::<Self>();

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
pub fn discover(interface: Arc<dyn NetInterface>) {
    let options = [
        // DHCP Discover
        DhcpOption::new(DhcpOptionType::MessageType, &[1]),
    ];

    let xid = rng::rand_u32();

    let _ = log!(transmit_dhcp(interface, xid, &options));
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

/// Main loop for the DHCP client thread, which listens for incoming DHCP packets and handles them
/// according to the DHCP state machine.
pub fn dhcp_thread() {
    let socket = SocketTable::open(68).expect("failed to open DHCP client port");

    loop {
        if let Ok((_ip, _port, packet)) = SocketTable::receive(socket) {
            let Some((header, options)) = BootpHeader::from_bytes_with_rest(&packet) else {
                continue;
            };

            if header.op != 2 {
                continue;
            }

            let chaddr = MacAddr(header.chaddr[..6].try_into().unwrap());
            let Some(interface) = net::find_interface_by_mac(chaddr) else {
                continue;
            };

            if options.len() < 4 || options[..4] != MAGIC_COOKIE {
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
                        request(
                            interface.clone(),
                            header.xid.get(),
                            server,
                            Ipv4Addr(header.yiaddr.0),
                        );
                    }
                }

                // DHCP Ack
                Some(5) => {
                    // TODO: all other options like subnet mask, gateway, DNS, lease time, etc.
                    interface.set_ip(Ipv4Addr(header.yiaddr.0));
                }

                _ => {}
            }
        }
    }
}
