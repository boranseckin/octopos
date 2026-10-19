use alloc::boxed::Box;
use alloc::string::ToString;
use alloc::sync::Arc;

use crate::net::interface::{self, InterfaceId};
use crate::net::ipv4::Ipv4Proto;
use crate::net::socket::{RecvQueue, SockAddr};
use crate::net::{self, Be, Ipv4Addr, NetError, NetworkHeader};
use crate::net::{route, source_addr};
use crate::param::NSOCKET;
use crate::spinlock::SpinLock;

const EPHEMERAL_PORT_START: u16 = 49152;
const EPHEMERAL_PORT_END: u16 = 65535;

pub const MAX_UDP_PAYLOAD: usize = 1024;

/// A bound UDP socket. The binding is fixed at creation; only the receive state is mutable.
/// The queue has a fixed maximum depth; if the queue is full, incoming datagrams for that port will
/// be dropped until space is available.
#[derive(Debug)]
pub struct UdpSocket {
    pub bound_ip: Ipv4Addr,
    pub bound_port: u16,
    pub bound_interface: Option<InterfaceId>,
    queue: RecvQueue,
}

impl UdpSocket {
    /// Binds a new socket and registers in the UDP table.
    ///
    /// - If the `ip` argument is `UNSPECIFIED`, it will match datagrams sent to any of the host's IP addresses.
    /// - If the `port` argument is `0`, it will assign an ephemeral port automatically.
    /// - If the `interface` argument is `Some`, the socket will only receive datagrams that arrived
    ///   on that interface. If `None`, it will receive datagrams from any interface.
    pub fn bind(
        ip: Ipv4Addr,
        port: u16,
        interface: Option<InterfaceId>,
    ) -> Result<Arc<Self>, NetError> {
        let mut table = UDP_TABLE.lock();
        log!(table.add(ip, port, interface))
    }

    /// Unregisters from the table, marks closed, wakes sleepers.
    pub fn close(self: &Arc<Self>) {
        {
            let mut table = UDP_TABLE.lock();
            table.remove(self);
        }

        self.queue.close();
    }

    pub fn send(&self, dest_ip: Ipv4Addr, dest_port: u16, buf: &[u8]) -> Result<(), NetError> {
        if self.queue.is_closed() {
            err!(NetError::BadSocket)
        }

        log!(transmit_udp(dest_ip, dest_port, self.bound_port, buf))
    }

    pub fn receive(&self) -> Result<(Ipv4Addr, u16, Box<[u8]>), NetError> {
        let (addr, payload) = try_log!(self.queue.receive());
        let SockAddr::Inet { ip, port } = addr;
        Ok((ip, port, payload))
    }

    fn deliver(&self, ip: Ipv4Addr, port: u16, payload: Box<[u8]>) {
        let addr = SockAddr::Inet { ip, port };
        self.queue.deliver(addr, payload);
    }
}

/// Global socket table, protected by a spinlock.
/// Each entry is either None (unused) or Some(SocketEntry) for a bound port.
#[derive(Debug)]
struct UdpTable {
    entries: [Option<Arc<UdpSocket>>; NSOCKET],
    next_ephemeral: u16,
}

static UDP_TABLE: SpinLock<UdpTable> = SpinLock::new(
    UdpTable {
        entries: [const { None }; NSOCKET],
        next_ephemeral: EPHEMERAL_PORT_START,
    },
    "udp_sockets",
);

impl UdpTable {
    /// Returns the next ephemeral port and advances the counter.
    /// The counter wraps within the range [`EPHEMERAL_PORT_START`, `EPHEMERAL_PORT_END`).
    fn next_ephemeral(&mut self) -> u16 {
        let port = self.next_ephemeral;
        self.next_ephemeral += 1;
        if self.next_ephemeral == EPHEMERAL_PORT_END {
            self.next_ephemeral = EPHEMERAL_PORT_START;
        }
        port
    }

    /// Determines if there is an interface overlap between the existing and new socket entries.
    fn interface_overlap(existing: Option<InterfaceId>, new: Option<InterfaceId>) -> bool {
        match (existing, new) {
            (None, _) | (_, None) => true,
            (Some(a), Some(b)) => a == b,
        }
    }

    /// Finds the index of the socket entry that matches the given destination IP, port and
    /// interface, preferring an exact match on both IP and port, but falling back to a wildcard
    /// match on the IP.
    ///
    /// Used for routing incoming packets.
    fn ingress_lookup(
        &self,
        ip: Ipv4Addr,
        port: u16,
        interface: Option<InterfaceId>,
    ) -> Option<Arc<UdpSocket>> {
        let matches = |s: &&Arc<UdpSocket>, target_ip: Ipv4Addr| {
            s.bound_port == port
                && s.bound_ip == target_ip
                && Self::interface_overlap(s.bound_interface, interface)
        };

        // prefer (ip == bound_ip, port, interface) match, fall back to wildcard 0.0.0.0:port
        self.entries
            .iter()
            .flatten()
            .find(|s| matches(s, ip))
            .or_else(|| {
                self.entries
                    .iter()
                    .flatten()
                    .find(|s| matches(s, Ipv4Addr::UNSPECIFIED))
            })
            .cloned()
    }

    /// Checks if the combination of (`ip`, `port`, `interface`) will conflict with an existing
    /// entry.
    ///
    /// New             Existing        Conflict
    /// IP:PORT      -> IP:PORT       : Yes
    /// IP:PORT      -> 0.0.0.0:PORT  : Yes
    /// IP:PORT      -> OTHER_IP:PORT : No
    /// 0.0.0.0:PORT -> x:PORT        : Yes
    /// IP:PORT      -> x:OTHER_PORT  : No
    ///
    /// Interface conflicts if either the existing or the new one is `None`, or they are the same.
    fn is_bind_conflict(&self, ip: Ipv4Addr, port: u16, interface: Option<InterfaceId>) -> bool {
        if ip == Ipv4Addr::UNSPECIFIED {
            self.entries.iter().flatten().any(|s| {
                s.bound_port == port && Self::interface_overlap(s.bound_interface, interface)
            })
        } else {
            self.entries.iter().flatten().any(|s| {
                s.bound_port == port
                    && (s.bound_ip == ip || s.bound_ip == Ipv4Addr::UNSPECIFIED)
                    && Self::interface_overlap(s.bound_interface, interface)
            })
        }
    }

    /// Resolves the port to bind: picks a free ephemeral port if `port == 0`, otherwise checks for
    /// a confglicting binding. Caller must hold the table lock.
    fn resolve_port(
        &mut self,
        ip: Ipv4Addr,
        port: u16,
        interface: Option<InterfaceId>,
    ) -> Result<u16, NetError> {
        // resolve the port to bind
        let bind_port = if port == 0 {
            // scan the ephemeral range for a port not already in use
            let mut current = self.next_ephemeral();
            let start = current;

            'ephemeral: loop {
                for entry in self.entries.iter().flatten() {
                    if entry.bound_port == current {
                        current = self.next_ephemeral();

                        if current == start {
                            // we've looped all the way around and found no free ports
                            err!(NetError::OutOfSocket);
                        }

                        continue 'ephemeral;
                    }
                }

                // found a free port, increment the counter for the future and break
                break current;
            }
        } else {
            // check if there is a conflicting binding
            if self.is_bind_conflict(ip, port, interface) {
                err!(NetError::PortInUse);
            }
            port
        };

        Ok(bind_port)
    }

    /// Resovles the port, creates the socket, and inserts it into a free slot, all under the
    /// caller's table lock so no other bind can claim the same port in between.
    fn add(
        &mut self,
        ip: Ipv4Addr,
        port: u16,
        interface: Option<InterfaceId>,
    ) -> Result<Arc<UdpSocket>, NetError> {
        let Some(id) = self.entries.iter().position(|e| e.is_none()) else {
            err!(NetError::TableFull);
        };

        let port = try_log!(self.resolve_port(ip, port, interface));

        let Ok(socket) = Arc::try_new(UdpSocket {
            bound_ip: ip,
            bound_port: port,
            bound_interface: interface,
            queue: RecvQueue::new(),
        }) else {
            err!(NetError::ResourceUnavailable);
        };

        self.entries[id] = Some(socket.clone());

        Ok(socket)
    }

    /// Removes a socket from the table.
    fn remove(&mut self, socket: &Arc<UdpSocket>) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.as_ref().is_some_and(|s| Arc::ptr_eq(s, socket)))
        {
            *entry = None;
        }
    }
}

/// UDP Header
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
pub struct UdpHeader {
    src_port: Be<u16>,
    dest_port: Be<u16>,
    len: Be<u16>,
    sum: Be<u16>,
}

/// The pseudo-header used for UDP checksum calculation, which includes fields from the IPv4 header
/// as well as the UDP header fields. This is not sent over the wire, but is required for the
/// checksum calculation as per RFC 768.
///
/// Why the protocol requires this for the checksum, I have no idea. It just adds more complexity to
/// everything but oh well...
#[derive(Debug, Clone, Copy)]
#[repr(C, packed)]
struct UdpPseudoHeader {
    src_ip: Ipv4Addr,
    dest_ip: Ipv4Addr,
    zero: u8,
    proto: u8,
    udp_len: Be<u16>,
}

unsafe impl NetworkHeader for UdpHeader {}
unsafe impl NetworkHeader for UdpPseudoHeader {}

impl UdpHeader {
    pub const SIZE: usize = size_of::<Self>();

    pub fn new(src_port: u16, dest_port: u16, payload_len: usize) -> Self {
        Self {
            src_port: Be::new(src_port),
            dest_port: Be::new(dest_port),
            len: Be::new((Self::SIZE + payload_len) as u16),
            sum: Be::new(0),
        }
    }

    fn calculate_checksum(&self, src_ip: Ipv4Addr, dest_ip: Ipv4Addr, payload: &[u8]) -> u16 {
        let mut header = *self;
        header.sum = Be::new(0);

        let pseudo_header = UdpPseudoHeader {
            src_ip,
            dest_ip,
            zero: 0,
            proto: Ipv4Proto::Udp as u8,
            udp_len: header.len,
        };

        net::internet_checksum(&[pseudo_header.as_bytes(), header.as_bytes(), payload])
    }

    pub fn add_checksum(mut self, src_ip: Ipv4Addr, dest_ip: Ipv4Addr, payload: &[u8]) -> Self {
        self.sum = Be::new(self.calculate_checksum(src_ip, dest_ip, payload));
        self
    }
}

/// Transmits a UDP datagram to the specified destination IP and port, with the given source port
/// and payload. This function constructs the UDP header, calculates the checksum, and then sends
/// the packet using the underlying `net::transmit` function.
fn transmit_udp(
    dest_ip: Ipv4Addr,
    dest_port: u16,
    src_port: u16,
    payload: &[u8],
) -> Result<(), NetError> {
    // UDP requires L3 routing information in the header for checksum calculation, so we need to do
    // the routing lookup before passing it to net::transmit().
    // To avoid double lookup, we are using net::transmit_with_route().
    let route = try_log!(route::best_route_for(dest_ip));

    let Some(interface) = interface::find_interface_by_id(route.interface_id) else {
        err!(NetError::NotConfigured);
    };

    let Some(src_ipv4) = source_addr(&route, &interface) else {
        err!(NetError::NotConfigured);
    };

    let header =
        UdpHeader::new(src_port, dest_port, payload.len()).add_checksum(src_ipv4, dest_ip, payload);

    log!(net::transmit_with_route(
        route,
        interface,
        dest_ip,
        Ipv4Proto::Udp,
        &[header.as_bytes(), payload]
    ))
}

/// Handles an incoming UDP datagram by parsing the header, validating it, and enqueuing the payload
/// into the receive queue of the appropriate socket based on the destination port. If no socket is
/// bound to the destination port, the datagram is dropped.
pub fn handle_udp(
    interface_id: InterfaceId,
    dest_ip: Ipv4Addr,
    src_ip: Ipv4Addr,
    req_data: &[u8],
) -> Result<(), NetError> {
    let Some((req_udp, req_data)) = UdpHeader::from_bytes_with_rest(req_data) else {
        err!(NetError::MalformedPacket)
    };

    // verify length
    let udp_len = req_udp.len.get() as usize;
    if udp_len < UdpHeader::SIZE || udp_len > req_data.len() + UdpHeader::SIZE {
        err!(NetError::MalformedPacket);
    }

    // truncate data to declared length in the header
    let payload_len = udp_len.saturating_sub(UdpHeader::SIZE);
    let req_data = &req_data[..payload_len.min(req_data.len())];

    let Some(socket) =
        UDP_TABLE
            .lock()
            .ingress_lookup(dest_ip, req_udp.dest_port.get(), Some(interface_id))
    else {
        // no socket is bound to this (ip, port), drop it
        return Ok(());
    };

    // do not return here if the queue is full, might as well wakeup the proc so that they can
    // process the full queue.
    socket.deliver(src_ip, req_udp.src_port.get(), req_data.into());

    Ok(())
}

/// Dumps the current state of the socket table for debugging purposes.
pub fn dump() {
    let table = UDP_TABLE.lock();

    println!("");
    for (id, entry) in table.entries.iter().enumerate() {
        if let Some(entry) = entry {
            println!(
                "socket {}: {}:{}/{}",
                id,
                entry.bound_ip,
                entry.bound_port,
                entry
                    .bound_interface
                    .map_or("any".to_string(), |i| i.to_string()),
            );
        }
    }
}
