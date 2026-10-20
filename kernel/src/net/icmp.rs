use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::abi::SockAddr;
use crate::net::ipv4::Ipv4Proto;
use crate::net::socket::RecvQueue;
use crate::net::{self, Be, Ipv4Addr, NetError, NetworkHeader};
use crate::param::NSOCKET;
use crate::spinlock::SpinLock;

const EPHEMERAL_IDENT_START: u16 = 1;
const EPHEMERAL_IDENT_END: u16 = u16::MAX;

/// Ping socket, only handles ICMP Echo.
/// Identified by its echo identifier, like a UDP port.
#[derive(Debug)]
pub struct PingSocket {
    pub ident: u16,
    queue: RecvQueue,
}

impl PingSocket {
    /// Bind a new socket and registers in the Ping table.
    /// `ident == 0` picks a free identifier.
    pub fn bind(ident: u16) -> Result<Arc<Self>, NetError> {
        let mut table = PING_TABLE.lock();
        log!(table.add(ident))
    }

    pub fn close(self: &Arc<Self>) {
        {
            let mut table = PING_TABLE.lock();
            table.remove(self);
        }

        self.queue.close();
    }

    /// Sends a Echo Request packet to the given destination IP.
    /// `buf` is a full ICMP message (header + payload) built by the user.
    ///
    /// The ident is overwritten for security, and the checksum is recalculated.
    pub fn send(&self, dest_ip: Ipv4Addr, buf: &[u8]) -> Result<(), NetError> {
        if self.queue.is_closed() {
            err!(NetError::BadSocket);
        }

        let Some((req_icmp, req_data)) = IcmpHeader::from_bytes_with_rest(buf) else {
            err!(NetError::MalformedPacket);
        };

        if req_icmp.r#type() != IcmpType::EchoRequest || req_icmp.code != 0 {
            err!(NetError::MalformedPacket);
        }

        // overwrite the identifier, so a user can't spoof replies meant for other sockets.
        // recalculate checksum, in case it was left as zero.
        let req_icmp = req_icmp.with_echo_ident(self.ident).add_checksum(req_data);

        log!(net::transmit(
            dest_ip,
            Ipv4Proto::Icmp,
            &[req_icmp.as_bytes(), req_data]
        ))
    }

    pub fn receive(&self) -> Result<(SockAddr, Box<[u8]>), NetError> {
        log!(self.queue.receive())
    }

    fn deliver(&self, from: SockAddr, payload: &[u8]) {
        self.queue.deliver(from, Box::from(payload));
    }

    pub fn set_receive_timeout(&self, ticks: usize) {
        self.queue.set_timeout(ticks);
    }
}

struct PingTable {
    entries: [Option<Arc<PingSocket>>; NSOCKET],
    next_ident: u16,
}

static PING_TABLE: SpinLock<PingTable> = SpinLock::new(
    PingTable {
        entries: [const { None }; NSOCKET],
        next_ident: EPHEMERAL_IDENT_START,
    },
    "ping_sockets",
);

impl PingTable {
    /// Returns the next ephemeral port and advances the counter.
    /// The counter wraps within the range [`EPHEMERAL_IDENT_START`, `EPHEMERAL_IDENT_END`).
    fn next_ident(&mut self) -> u16 {
        let port = self.next_ident;
        self.next_ident += 1;
        if self.next_ident == EPHEMERAL_IDENT_END {
            self.next_ident = EPHEMERAL_IDENT_START;
        }
        port
    }

    /// Resolves the ident to bind: picks a free ephemeral ident if `ident == 0`, otherwise checks
    /// for a confglicting binding. Caller must hold the table lock.
    fn resolve_ident(&mut self, ident: u16) -> Result<u16, NetError> {
        // resolve the ident to bind
        let bind_ident = if ident == 0 {
            // scan the ephemeral range for a ident not already in use
            let mut current = self.next_ident();
            let start = current;

            'ephemeral: loop {
                for entry in self.entries.iter().flatten() {
                    if entry.ident == current {
                        current = self.next_ident();

                        if current == start {
                            // we've looped all the way around and found no free ident
                            err!(NetError::OutOfSocket);
                        }

                        continue 'ephemeral;
                    }
                }

                // found a free ident, increment the counter for the future and break
                break current;
            }
        } else {
            // check if there is a conflicting binding
            if self.entries.iter().flatten().any(|e| e.ident == ident) {
                err!(NetError::PortInUse)
            }
            ident
        };

        Ok(bind_ident)
    }

    /// Finds the index of the socket entry that matches the given destination ident.
    /// Used for routing incoming packets.
    fn ingress_lookup(&self, ident: u16) -> Option<Arc<PingSocket>> {
        self.entries
            .iter()
            .flatten()
            .find(|s| s.ident == ident)
            .cloned()
    }

    /// Resovles the ident, creates the socket, and inserts it into a free slot, all under the
    /// caller's table lock so no other bind can claim the same port in between.
    fn add(&mut self, ident: u16) -> Result<Arc<PingSocket>, NetError> {
        let Some(id) = self.entries.iter().position(|e| e.is_none()) else {
            err!(NetError::TableFull);
        };

        let ident = try_log!(self.resolve_ident(ident));

        let Ok(socket) = Arc::try_new(PingSocket {
            ident,
            queue: RecvQueue::new(),
        }) else {
            err!(NetError::ResourceUnavailable);
        };

        self.entries[id] = Some(socket.clone());

        Ok(socket)
    }

    /// Removes a socket from the table.
    fn remove(&mut self, socket: &Arc<PingSocket>) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.as_ref().is_some_and(|s| Arc::ptr_eq(s, socket)))
        {
            *entry = None;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum IcmpType {
    EchoReply = 0,
    EchoRequest = 8,
    Unknown = u8::MAX,
}

impl From<u8> for IcmpType {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::EchoReply,
            8 => Self::EchoRequest,
            _ => Self::Unknown,
        }
    }
}

/// ICMP Header
/// https://en.wikipedia.org/wiki/Internet_Control_Message_Protocol#Header
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct IcmpHeader {
    /// Type
    r#type: u8,
    /// Code
    code: u8,
    /// Checksum
    sum: Be<u16>,
    /// Rest of Header (varies by type/code)
    rest: Be<u32>,
}

impl IcmpHeader {
    fn r#type(&self) -> IcmpType {
        IcmpType::from(self.r#type)
    }

    fn echo_ident(&self) -> u16 {
        (self.rest.get() >> 16) as u16
    }

    #[must_use]
    fn with_echo_ident(mut self, ident: u16) -> Self {
        self.rest = Be::new((self.rest.get() & 0xFFFF) | ((ident as u32) << 16));
        self
    }

    fn new_echo_reply(request: &IcmpHeader) -> Self {
        Self {
            r#type: IcmpType::EchoReply as u8,
            code: 0,
            sum: Be::new(0), // to be filled later
            rest: request.rest,
        }
    }

    fn calculate_checksum(&self, data: &[u8]) -> u16 {
        let mut header = *self;
        header.sum = Be::new(0);
        net::internet_checksum(&[header.as_bytes(), data])
    }

    #[must_use]
    fn add_checksum(mut self, data: &[u8]) -> Self {
        self.sum = Be::new(self.calculate_checksum(data));
        self
    }
}

unsafe impl NetworkHeader for IcmpHeader {}

/// Handles an incoming ICMP packet.
/// If it's an Echo Request, sends back an Echo Reply.
/// Otherwise, ignores it.
pub fn handle_icmp(src_ip: Ipv4Addr, req_data: &[u8]) -> Result<(), NetError> {
    let Some((header, payload)) = IcmpHeader::from_bytes_with_rest(req_data) else {
        err!(NetError::MalformedPacket)
    };

    if header.calculate_checksum(payload) != header.sum.get() {
        err!(NetError::ChecksumFailed)
    }

    match header.r#type() {
        IcmpType::EchoRequest => {
            let icmp = IcmpHeader::new_echo_reply(header).add_checksum(payload);

            log!(net::transmit(
                src_ip,
                Ipv4Proto::Icmp,
                &[icmp.as_bytes(), payload]
            ))
        }
        IcmpType::EchoReply => {
            let Some(socket) = PING_TABLE.lock().ingress_lookup(header.echo_ident()) else {
                return Ok(());
            };

            // use ident as the port
            let addr = SockAddr::inet(src_ip, header.echo_ident());
            socket.deliver(addr, req_data);

            Ok(())
        }
        IcmpType::Unknown => Ok(()),
    }
}
