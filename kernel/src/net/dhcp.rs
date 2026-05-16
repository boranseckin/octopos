use alloc::vec::Vec;

use crate::net::eth::{EtherType, EthernetHeader};
use crate::net::interface::{self, InterfaceId};
use crate::net::ipv4::{Ipv4Header, Ipv4Proto};
use crate::net::route::{self, RouteEntry, RouteOwner};
use crate::net::udp::{SocketTable, UdpHeader};
use crate::net::{Be, Ipv4Addr, Ipv4Config, MacAddr, NetError, NetworkHeader};
use crate::rng;

/// Vendor Information "Magic Cookie"
/// https://www.rfc-editor.org/rfc/rfc1497
const MAGIC_COOKIE: [u8; 4] = [0x63, 0x82, 0x53, 0x63];

#[derive(Debug, Clone, Copy)]
enum DhcpError {
    MalformedPacket,
    TruncatedOption,
    UnknownMessageType(u8),
    InvalidMessageType,
    MissingServerIdentifier,
    MissingSubnetMask,
    MissingMagicCookie,
    InvalidMagicCookie,
    TransactionMismatch,
    ServerMismatch,
    AddressMismatch,
    TransportError,
}

impl core::fmt::Display for DhcpError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MalformedPacket => write!(f, "malformed packet"),
            Self::TruncatedOption => write!(f, "truncated option"),
            Self::UnknownMessageType(v) => write!(f, "unknown message type ({})", v),
            Self::InvalidMessageType => write!(f, "invalid message type"),
            Self::MissingServerIdentifier => write!(f, "missing Server Identifier option"),
            Self::MissingSubnetMask => write!(f, "missing Subnet Mask option"),
            Self::MissingMagicCookie => write!(f, "missing magic cookie in options"),
            Self::InvalidMagicCookie => write!(f, "invalid magic cookie in options"),
            Self::TransactionMismatch => write!(f, "transaction ID does not match"),
            Self::ServerMismatch => write!(f, "server identifier does not match"),
            Self::AddressMismatch => write!(f, "offered IP address does not match"),
            Self::TransportError => write!(f, "error with transport layer"),
        }
    }
}

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

/// DHCP message types as defined in RFC 2132
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DhcpMessageType {
    Discover = 1,
    Offer = 2,
    Request = 3,
    Decline = 4,
    Ack = 5,
    Nak = 6,
    Release = 7,
    Inform = 8,
}

impl TryFrom<u8> for DhcpMessageType {
    type Error = DhcpError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Discover),
            2 => Ok(Self::Offer),
            3 => Ok(Self::Request),
            4 => Ok(Self::Decline),
            5 => Ok(Self::Ack),
            6 => Ok(Self::Nak),
            7 => Ok(Self::Release),
            8 => Ok(Self::Inform),
            _ => Err(DhcpError::UnknownMessageType(value)),
        }
    }
}

/// DHCP options as defined in RFC 2132, with some common options implemented.
#[derive(Debug, Clone)]
enum DhcpOption<'a> {
    MessageType(DhcpMessageType),
    ServerIdentifier(Ipv4Addr),
    AddressRequest(Ipv4Addr),
    SubnetMask(Ipv4Addr),
    Router(&'a [u8]),
    LeaseTime(u32),
    DnsServers(&'a [u8]),

    Unknown { code: u8, data: &'a [u8] },
}

impl<'a> DhcpOption<'a> {
    /// Returns the DHCP option code for this option.
    fn to_code(&self) -> u8 {
        match self {
            DhcpOption::MessageType(_) => 53,
            DhcpOption::ServerIdentifier(_) => 54,
            DhcpOption::AddressRequest(_) => 50,
            DhcpOption::SubnetMask(_) => 1,
            DhcpOption::Router(_) => 3,
            DhcpOption::LeaseTime(_) => 51,
            DhcpOption::DnsServers(_) => 6,
            DhcpOption::Unknown { code, .. } => *code,
        }
    }

    /// Returns the length of the data for this option, not including the code and length fields.
    fn data_len(&self) -> usize {
        match self {
            DhcpOption::MessageType(_) => 1,
            DhcpOption::ServerIdentifier(_) => 4,
            DhcpOption::AddressRequest(_) => 4,
            DhcpOption::SubnetMask(_) => 4,
            DhcpOption::Router(ipv4_addrs) => ipv4_addrs.len(),
            DhcpOption::LeaseTime(_) => 4,
            DhcpOption::DnsServers(ipv4_addrs) => ipv4_addrs.len(),
            DhcpOption::Unknown { code: _, data } => data.len(),
        }
    }

    /// Appends the option to the given buffer at the specified offset, updating the offset
    /// accordingly.
    ///
    /// This mirrors `NetworkHeader`'s `append_to()` method.
    fn append_to(&self, buf: &mut [u8], offset: &mut usize) {
        buf[*offset] = self.to_code();
        *offset += 1;
        buf[*offset] = self.data_len() as u8;
        *offset += 1;

        match self {
            DhcpOption::MessageType(ty) => {
                buf[*offset] = *ty as u8;
                *offset += 1;
            }

            DhcpOption::ServerIdentifier(ip)
            | DhcpOption::AddressRequest(ip)
            | DhcpOption::SubnetMask(ip) => {
                buf[*offset..*offset + 4].copy_from_slice(&ip.0);
                *offset += 4;
            }

            DhcpOption::Router(slice) | DhcpOption::DnsServers(slice) => {
                buf[*offset..*offset + slice.len()].copy_from_slice(slice);
                *offset += slice.len();
            }

            DhcpOption::LeaseTime(lease) => {
                buf[*offset..*offset + 4].copy_from_slice(&lease.to_be_bytes());
                *offset += 4;
            }

            DhcpOption::Unknown { code: _, data } => {
                buf[*offset..*offset + data.len()].copy_from_slice(data);
                *offset += data.len();
            }
        }
    }

    /// Decodes a DHCP option from the given code and data bytes.
    ///
    /// Returns `Ok(DhcpOption::Unknown)` for unsupported option codes, and returns an `Err` if the
    /// data is malformed for the expected option type.
    fn decode(code: u8, data: &'a [u8]) -> Result<Self, DhcpError> {
        match code {
            1 => Ok(Self::SubnetMask(try_log!(
                Ipv4Addr::try_from(data).map_err(|_| DhcpError::MalformedPacket)
            ))),

            3 => {
                if !data.len().is_multiple_of(4) {
                    err!(DhcpError::MalformedPacket);
                }

                Ok(Self::Router(data))
            }

            6 => {
                if !data.len().is_multiple_of(4) {
                    err!(DhcpError::MalformedPacket);
                }

                Ok(Self::DnsServers(data))
            }

            51 => {
                if data.len() != 4 {
                    err!(DhcpError::MalformedPacket);
                }

                Ok(Self::LeaseTime(u32::from_be_bytes(
                    data.try_into().expect("data length already checked"),
                )))
            }

            53 => {
                if data.len() != 1 {
                    err!(DhcpError::MalformedPacket);
                }

                Ok(Self::MessageType(try_log!(DhcpMessageType::try_from(
                    data[0]
                ))))
            }

            54 => Ok(Self::ServerIdentifier(try_log!(
                Ipv4Addr::try_from(data).map_err(|_| DhcpError::MalformedPacket)
            ))),

            _ => Ok(Self::Unknown { code, data }),
        }
    }
}

/// Reader type for iterating over DHCP options in a received packet, which are encoded as TLV
/// (Type-Length-Value) fields until the End Option (0xFF) is encountered.
#[derive(Debug, Clone)]
struct DhcpOptionIter<'a>(&'a [u8]);

impl<'a> Iterator for DhcpOptionIter<'a> {
    type Item = Result<DhcpOption<'a>, DhcpError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() {
            return None;
        }

        match self.0[0] {
            0x00 => {
                // Pad Option
                self.0 = &self.0[1..];
                self.next()
            }

            0xFF => {
                // End Option
                self.0 = &self.0[1..];
                None
            }

            _ => {
                // must have at least a code byte and a length byte.
                if self.0.len() < 2 {
                    return Some(Err(DhcpError::MalformedPacket));
                }

                let code = self.0[0];
                let length = self.0[1] as usize;

                // must have enough bytes for the data according to the length field.
                if self.0.len() < 2 + length {
                    return Some(Err(DhcpError::TruncatedOption));
                }

                let data = &self.0[2..2 + length];

                // move the slice forward for the next iteration
                self.0 = &self.0[2 + length..];

                Some(DhcpOption::decode(code, data))
            }
        }
    }
}

/// Parsed DHCP packet with the BOOTP header and a list of options.
#[derive(Debug, Clone)]
struct DhcpPacket<'a> {
    header: BootpHeader,
    options: Vec<DhcpOption<'a>>,
}

impl<'a> DhcpPacket<'a> {
    /// Returns the DHCP message type from the options, if present.
    fn message_type(&self) -> Option<DhcpMessageType> {
        self.options.iter().find_map(|op| match op {
            DhcpOption::MessageType(ty) => Some(*ty),
            _ => None,
        })
    }

    /// Returns the Server Identifier option from the options, if present.
    fn server_identifier(&self) -> Option<Ipv4Addr> {
        self.options.iter().find_map(|op| match op {
            DhcpOption::ServerIdentifier(ip) => Some(*ip),
            _ => None,
        })
    }

    /// Returns the Subnet Mask option from the options, if present.
    fn subnet_mask(&self) -> Option<Ipv4Addr> {
        self.options.iter().find_map(|op| match op {
            DhcpOption::SubnetMask(mask) => Some(*mask),
            _ => None,
        })
    }

    /// Returns the Router option from the options, if present.
    /// If multiple routers are present, returns the first one.
    fn router(&self) -> Option<Ipv4Addr> {
        self.options.iter().find_map(|op| match op {
            DhcpOption::Router(routers) => {
                if routers.len() < 4 {
                    return None;
                }

                Ipv4Addr::try_from(&routers[..4]).ok()
            }
            _ => None,
        })
    }

    /// Returns the Lease Time option from the options, if present.
    fn lease_time(&self) -> Option<u32> {
        self.options.iter().find_map(|op| match op {
            DhcpOption::LeaseTime(lease) => Some(*lease),
            _ => None,
        })
    }

    /// Parses a raw DHCP packet from the given byte slice, extracting the BOOTP header and options.
    fn parse(packet: &'a [u8]) -> Result<DhcpPacket<'a>, DhcpError> {
        let Some((header, options_data)) = BootpHeader::from_bytes_with_rest(packet) else {
            err!(DhcpError::MalformedPacket);
        };

        // must be a BOOTREPLY (op=2) since this is a packet received from the server.
        if header.op != 2 {
            err!(DhcpError::MalformedPacket);
        }

        // options must start with the magic cookie to be valid
        if options_data.len() < 4 {
            err!(DhcpError::MissingMagicCookie);
        }

        if options_data[..4] != MAGIC_COOKIE {
            err!(DhcpError::InvalidMagicCookie);
        }

        let mut options = Vec::new();

        for option in DhcpOptionIter(&options_data[4..]) {
            options.push(try_log!(option));
        }

        Ok(Self {
            header: *header,
            options,
        })
    }
}

/// Parsed DHCP Offer message
#[derive(Debug, Clone)]
struct DhcpOffer {
    xid: u32,

    // required
    server_id: Ipv4Addr,
    offered_ip: Ipv4Addr,
    subnet_mask: Ipv4Addr,

    // optional
    _router: Option<Ipv4Addr>,
    _lease_time: Option<u32>,
}

impl TryFrom<DhcpPacket<'_>> for DhcpOffer {
    type Error = DhcpError;

    fn try_from(value: DhcpPacket) -> Result<Self, Self::Error> {
        if value.message_type() != Some(DhcpMessageType::Offer) {
            err!(DhcpError::InvalidMessageType);
        }

        let Some(server_id) = value.server_identifier() else {
            err!(DhcpError::MissingServerIdentifier);
        };

        let Some(subnet_mask) = value.subnet_mask() else {
            err!(DhcpError::MissingSubnetMask);
        };

        Ok(Self {
            xid: value.header.xid.get(),
            server_id,
            offered_ip: value.header.yiaddr,
            subnet_mask,
            _router: value.router(),
            _lease_time: value.lease_time(),
        })
    }
}

/// Parsed DHCP Ack message
#[derive(Debug, Clone)]
struct DhcpAck {
    xid: u32,

    // required
    server_id: Ipv4Addr,
    assigned_ip: Ipv4Addr,
    subnet_mask: Ipv4Addr,

    // optional
    router: Option<Ipv4Addr>,
}

impl TryFrom<DhcpPacket<'_>> for DhcpAck {
    type Error = DhcpError;

    fn try_from(value: DhcpPacket) -> Result<Self, Self::Error> {
        if value.message_type() != Some(DhcpMessageType::Ack) {
            err!(DhcpError::InvalidMessageType);
        }

        let Some(server_id) = value.server_identifier() else {
            err!(DhcpError::MissingServerIdentifier);
        };

        let Some(subnet_mask) = value.subnet_mask() else {
            err!(DhcpError::MissingSubnetMask);
        };

        Ok(Self {
            xid: value.header.xid.get(),
            server_id,
            assigned_ip: value.header.yiaddr,
            subnet_mask,
            router: value.router(),
        })
    }
}

/// State machine for DHCP negotiation on a single interface.
#[derive(Debug, Clone)]
enum DhcpState {
    Discovering { xid: u32 },
    Requesting(DhcpOffer),
    Bound(DhcpAck),
}

impl DhcpState {
    /// Returns the transaction ID for the current state.
    fn xid(&self) -> u32 {
        match self {
            DhcpState::Discovering { xid } => *xid,
            DhcpState::Requesting(dhcp_offer) => dhcp_offer.xid,
            DhcpState::Bound(dhcp_ack) => dhcp_ack.xid,
        }
    }

    /// Begins the DHCP negotiation process by sending a Discover message and transitioning to the
    /// `Discovering` state with the generated transaction ID.
    fn begin(interface_id: InterfaceId) -> Result<Self, DhcpError> {
        let xid = try_log!(discover(interface_id));

        Ok(DhcpState::Discovering { xid })
    }

    /// Handles a received DHCP Offer message, validating the transaction ID and sending a Request
    /// message in response, then transitioning to the `Requesting` state.
    fn on_offer(&self, interface_id: InterfaceId, offer: DhcpOffer) -> Result<Self, DhcpError> {
        match self {
            Self::Discovering { xid } if *xid == offer.xid => {
                try_log!(request(interface_id, &offer));

                Ok(Self::Requesting(offer))
            }
            _ => Err(DhcpError::TransactionMismatch),
        }
    }

    /// Handles a received DHCP Ack message, validating the transaction ID and offered parameters,
    /// then configuring the interface with the assigned IP address and subnet mask, adding a
    /// default route if a router is provided, and transitioning to the `Bound` state.
    fn on_ack(&self, interface_id: InterfaceId, ack: DhcpAck) -> Result<Self, DhcpError> {
        match self {
            Self::Requesting(offer) if offer.xid == ack.xid => {
                if offer.server_id != ack.server_id {
                    err!(DhcpError::ServerMismatch);
                }

                if offer.offered_ip != ack.assigned_ip || offer.subnet_mask != ack.subnet_mask {
                    err!(DhcpError::AddressMismatch);
                }

                try_log!(
                    interface::set_interface_ipv4(
                        interface_id,
                        Some(Ipv4Config {
                            addr: ack.assigned_ip,
                            prefix_len: Ipv4Addr::mask_to_prefix_len(ack.subnet_mask).unwrap(),
                        }),
                    )
                    .map_err(|_| DhcpError::TransportError)
                );

                // add gateway if router option is provided
                if let Some(router) = ack.router {
                    route::upsert_route(RouteEntry {
                        dest_ip: Ipv4Addr::UNSPECIFIED,
                        prefix_len: 0,
                        gateway: Some(router),
                        interface_id,
                        metric: 100, // default metric for DHCP routes, higher than other routes
                        owner: RouteOwner::Dhcp,
                    });
                }

                Ok(Self::Bound(ack))
            }

            _ => Err(DhcpError::TransactionMismatch),
        }
    }
}

/// Transmits a DHCP packet with the given transaction ID and options on the specified interface.
///
/// This function constructs the entire Ethernet + IPv4 + UDP + BOOTP + DHCP packet from scratch
/// since we cannot use UDP's `SocketTable::send()` without an IP address, and DHCP requires
/// specific handling of broadcast packets and options.
fn transmit_dhcp(
    interface_id: InterfaceId,
    xid: u32,
    options: &[DhcpOption],
) -> Result<(), NetError> {
    // calculate the total size of the DHCP options (+1 for the END option)
    // 1 byte for ID + 1 byte for length + data length
    let options_size = options.iter().map(|o| o.data_len() + 2).sum::<usize>() + 1;
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

    let Some(interface) = interface::find_interface_by_id(interface_id) else {
        err!(NetError::InterfaceNotFound)
    };

    EthernetHeader::new(MacAddr::BROADCAST, interface.config.mac, EtherType::Ipv4)
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
    // UDP checksum is ignored for DHCP, no need to calculate
    UdpHeader::new(68, 67, dhcp_size).append_to(&mut packet, &mut offset);

    let mut header = BootpHeader::new_request();
    // copy our MAC address into the header
    header.chaddr[..6].copy_from_slice(&interface.config.mac.0);
    // set the broadcast flag since we don't have an IP address yet
    header.flags = Be::new(0x8000u16);
    // set the transaction ID
    header.xid = Be::new(xid);

    header.append_to(&mut packet, &mut offset);

    packet[offset..offset + MAGIC_COOKIE.len()].copy_from_slice(&MAGIC_COOKIE);
    offset += MAGIC_COOKIE.len();

    options
        .iter()
        .for_each(|op| op.append_to(&mut packet, &mut offset));

    packet[offset] = 0xFF; // End Option
    offset += 1;

    // use device directly since we created the whole packet from scratch
    log!(interface.device.transmit(&packet[..offset]))
}

/// Sends a DHCP Discover to find available DHCP servers and obtain an IP address lease.
///
/// The client generates a random transaction ID to match the Discover with subsequent messages from
/// the server.
fn discover(interface_id: InterfaceId) -> Result<u32, DhcpError> {
    let options = [
        // DHCP Discover
        DhcpOption::MessageType(DhcpMessageType::Discover),
    ];

    let xid = rng::rand_u32();

    match log!(transmit_dhcp(interface_id, xid, &options)) {
        Ok(_) => Ok(xid),
        Err(_) => Err(DhcpError::TransportError),
    }
}

/// Sends a DHCP Request in response to a received DHCP Offer.
fn request(interface_id: InterfaceId, offer: &DhcpOffer) -> Result<(), DhcpError> {
    let options = [
        // DHCP Request
        DhcpOption::MessageType(DhcpMessageType::Request),
        // Request the same IP address that was offered to us
        DhcpOption::AddressRequest(offer.offered_ip),
        // Include the Server Identifier to specify which server we're accepting the offer from
        DhcpOption::ServerIdentifier(offer.server_id),
    ];

    log!(transmit_dhcp(interface_id, offer.xid, &options)).map_err(|_| DhcpError::TransportError)
}

/// Thread for handling DHCP negotiation for the given interface.
///
// TODO: implement retries and backoff for failed attempts
pub fn dhcp_thread(interface_id: InterfaceId) {
    let socket = SocketTable::open(Ipv4Addr::UNSPECIFIED, 68, Some(interface_id))
        .expect("failed to open DHCP client port");

    let Ok(mut state) = log!(DhcpState::begin(interface_id)) else {
        SocketTable::close(socket);
        return;
    };

    while let Ok((_ip, _port, packet)) = log!(SocketTable::receive(socket)) {
        let Ok(dhcp) = log!(DhcpPacket::parse(&packet)) else {
            continue;
        };

        if dhcp.header.xid.get() != state.xid() {
            continue;
        }

        match dhcp.message_type() {
            Some(DhcpMessageType::Offer) => {
                let Ok(offer) = log!(DhcpOffer::try_from(dhcp)) else {
                    continue;
                };

                state = match state.on_offer(interface_id, offer) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
            }

            Some(DhcpMessageType::Ack) => {
                let Ok(ack) = log!(DhcpAck::try_from(dhcp)) else {
                    continue;
                };

                state = match state.on_ack(interface_id, ack) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
            }

            _ => {}
        }
    }

    SocketTable::close(socket);
}
