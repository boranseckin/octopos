use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;

use crate::net::icmp::PingSocket;
use crate::net::packet::PacketSocket;
use crate::net::udp::UdpSocket;
use crate::net::{InterfaceId, Ipv4Addr, MacAddr, NetError, interface};
use crate::proc::{self, Channel, Pod};
use crate::spinlock::SpinLock;
use crate::trap::TICKS;

const MAX_RECV_QUEUE_DEPTH: usize = 16;

const ANY_INTERFACE: u16 = u16::MAX;
const ANY_ETHER_TYPE: u16 = u16::MAX;

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    Udp = 1,
    Ping = 2,
    Packet = 3,
}

impl TryFrom<usize> for SocketType {
    type Error = NetError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Udp),
            2 => Ok(Self::Ping),
            3 => Ok(Self::Packet),
            _ => Err(NetError::BadSocket),
        }
    }
}

/// Address of a socket endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SockAddr {
    /// IPv4 address and port, used by UDP sockets.
    Inet { ip: Ipv4Addr, port: u16 },

    /// Link layer endpoint, used by packet sockets.
    Link {
        /// None = any interface
        interface: Option<InterfaceId>,
        /// None = every EtherType
        ether_type: Option<u16>,
        /// Peer MAC. Filled in on receive, ignored on bind and send.
        mac: MacAddr,
    },
}

impl SockAddr {
    pub const fn inet(ip: Ipv4Addr, port: u16) -> Self {
        Self::Inet { ip, port }
    }

    pub const fn any_inet() -> Self {
        Self::Inet {
            ip: Ipv4Addr::UNSPECIFIED,
            port: 0,
        }
    }

    pub const fn any_inet_with_port(port: u16) -> Self {
        Self::Inet {
            ip: Ipv4Addr::UNSPECIFIED,
            port,
        }
    }

    pub const fn link(interface: Option<InterfaceId>, ether_type: Option<u16>) -> SockAddr {
        Self::Link {
            interface,
            ether_type,
            mac: MacAddr::UNSPECIFIED,
        }
    }

    pub const fn any_link() -> Self {
        Self::Link {
            interface: None,
            ether_type: None,
            mac: MacAddr::UNSPECIFIED,
        }
    }
}

/// Wire format of a socket address shared with user space.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct RawSockAddr {
    /// 1 = inet, 2 = link
    pub family: u16,
    /// inet: port(2) addr(4) pad(8)
    /// link: interface(2) ether_type(2) mac(6) pad(4)
    pub data: [u8; 14],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct RawInetAddr {
    port: u16,
    ipv4: [u8; 4],
    _pad: [u8; 8],
}

const _: () = assert!(size_of::<RawInetAddr>() == 14);

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
struct RawLinkAddr {
    /// ANY_INTERFACE = any
    interface: u16,
    /// ANY_ETHER_TYPE = any
    ether_type: u16,
    mac: [u8; 6],
    _pad: [u8; 4],
}

const _: () = assert!(size_of::<RawLinkAddr>() == 14);

// Safety: RawSockAddr is repr(C), with no padding, and with primitive types.
unsafe impl Pod for RawSockAddr {}

impl From<SockAddr> for RawSockAddr {
    fn from(value: SockAddr) -> Self {
        // # Safety: Raw address structs consist of only primitive types with no implicit padding.
        match value {
            SockAddr::Inet { ip, port } => {
                let raw = RawInetAddr {
                    port,
                    ipv4: ip.0,
                    ..Default::default()
                };
                Self {
                    family: 1,
                    data: unsafe { core::mem::transmute::<RawInetAddr, [u8; 14]>(raw) },
                }
            }

            SockAddr::Link {
                interface,
                ether_type,
                mac,
            } => {
                let raw = RawLinkAddr {
                    interface: interface.map_or(ANY_INTERFACE, |i| i.0),
                    ether_type: ether_type.unwrap_or(ANY_ETHER_TYPE),
                    mac: mac.0,
                    ..Default::default()
                };

                Self {
                    family: 2,
                    data: unsafe { core::mem::transmute::<RawLinkAddr, [u8; 14]>(raw) },
                }
            }
        }
    }
}

impl TryFrom<RawSockAddr> for SockAddr {
    type Error = NetError;

    fn try_from(value: RawSockAddr) -> Result<Self, Self::Error> {
        // # Safety: Raw address structs consist of only primitive types with no implicit padding.
        match value.family {
            1 => {
                let raw: RawInetAddr = unsafe { core::mem::transmute(value.data) };

                Ok(Self::Inet {
                    ip: Ipv4Addr::from(raw.ipv4),
                    port: raw.port,
                })
            }

            2 => {
                let raw: RawLinkAddr = unsafe { core::mem::transmute(value.data) };

                Ok(Self::Link {
                    interface: if raw.interface == ANY_INTERFACE {
                        None
                    } else {
                        Some(InterfaceId(raw.interface))
                    },
                    ether_type: if raw.ether_type == ANY_ETHER_TYPE {
                        None
                    } else {
                        Some(raw.ether_type)
                    },
                    mac: MacAddr(raw.mac),
                })
            }

            _ => Err(NetError::InvalidAddress),
        }
    }
}

/// Handle to a socket of any type. Cloning refers to the same underlying socket.
/// The socket stays bound until `close()` is called, regardles of how many handles exists.
#[derive(Debug, Clone)]
pub enum Socket {
    Udp(Arc<UdpSocket>),
    Ping(Arc<PingSocket>),
    Packet(Arc<PacketSocket>),
}

impl Socket {
    pub fn open_udp(ip: Ipv4Addr, port: u16) -> Result<Self, NetError> {
        Ok(Self::Udp(try_log!(UdpSocket::bind(ip, port, None))))
    }

    pub fn open_ping(ident: u16) -> Result<Self, NetError> {
        Ok(Self::Ping(try_log!(PingSocket::bind(ident))))
    }

    pub fn open_packet(
        interface: Option<InterfaceId>,
        ether_type: Option<u16>,
    ) -> Result<Self, NetError> {
        Ok(Self::Packet(try_log!(PacketSocket::bind(
            interface, ether_type
        ))))
    }

    pub fn close(&self) {
        match self {
            Socket::Udp(udp_socket) => udp_socket.close(),
            Socket::Ping(ping_socket) => ping_socket.close(),
            Socket::Packet(packet_socket) => packet_socket.close(),
        }
    }

    pub fn set_receive_timeout(&self, ticks: usize) {
        match self {
            Socket::Udp(udp_socket) => udp_socket.set_receive_timeout(ticks),
            Socket::Ping(ping_socket) => ping_socket.set_receive_timeout(ticks),
            Socket::Packet(packet_socket) => packet_socket.set_receive_timeout(ticks),
        }
    }

    pub fn local_addr(&self) -> SockAddr {
        match self {
            Socket::Udp(udp_socket) => SockAddr::Inet {
                ip: udp_socket.bound_ip,
                port: udp_socket.bound_port,
            },

            Socket::Ping(ping_socket) => SockAddr::Inet {
                ip: Ipv4Addr::UNSPECIFIED,
                port: ping_socket.ident,
            },

            Socket::Packet(packet_socket) => SockAddr::Link {
                interface: packet_socket.interface,
                ether_type: packet_socket.ether_type,
                mac: packet_socket
                    .interface
                    .and_then(interface::find_interface_by_id)
                    .map_or(MacAddr::UNSPECIFIED, |i| i.config.mac),
            },
        }
    }

    pub fn send(&self, remote_addr: SockAddr, buf: &[u8]) -> Result<(), NetError> {
        match self {
            Socket::Udp(udp_socket) => {
                let SockAddr::Inet { ip, port } = remote_addr else {
                    err!(NetError::InvalidAddress)
                };

                log!(udp_socket.send(ip, port, buf))
            }

            Socket::Ping(ping_socket) => {
                let SockAddr::Inet { ip, port: _ } = remote_addr else {
                    err!(NetError::InvalidAddress)
                };

                log!(ping_socket.send(ip, buf))
            }

            Socket::Packet(packet_socket) => {
                let SockAddr::Link {
                    interface,
                    ether_type: _,
                    mac: _,
                } = remote_addr
                else {
                    err!(NetError::InvalidAddress)
                };

                log!(packet_socket.send(interface, buf))
            }
        }
    }

    pub fn receive(&self) -> Result<(SockAddr, Box<[u8]>), NetError> {
        match self {
            Socket::Udp(udp_socket) => log!(udp_socket.receive()),
            Socket::Ping(ping_socket) => log!(ping_socket.receive()),
            Socket::Packet(packet_socket) => log!(packet_socket.receive()),
        }
    }
}

#[derive(Debug)]
pub struct RecvQueue {
    inner: SpinLock<RecvQueueInner>,
}

#[derive(Debug)]
struct RecvQueueInner {
    entries: VecDeque<(SockAddr, Box<[u8]>)>, // bound by MAX_RECV_QUEUE_DEPTH
    closed: bool,
    /// Receive timeout in ticks. 0 = block forever.
    timeout: usize,
}

impl RecvQueue {
    /// Creates a new RecvQueue with capacity `MAX_RECV_QUEUE_DEPTH`.
    pub fn new() -> Self {
        Self {
            inner: SpinLock::new(
                RecvQueueInner {
                    entries: VecDeque::with_capacity(MAX_RECV_QUEUE_DEPTH),
                    closed: false,
                    timeout: 0,
                },
                "recv_queue",
            ),
        }
    }

    pub fn set_timeout(&self, ticks: usize) {
        self.inner.lock().timeout = ticks;
    }

    /// Delivers a packet to the RecvQueue.
    /// Drops the packet if the queue is full or the socket is closed.
    pub fn deliver(&self, from: SockAddr, payload: Box<[u8]>) {
        let mut inner = self.inner.lock();

        if inner.closed || inner.entries.len() >= MAX_RECV_QUEUE_DEPTH {
            return;
        }

        inner.entries.push_back((from, payload));

        proc::wakeup(self.channel());
    }

    /// Receives a packet from the RecvQueue.
    /// Sleeps until a packet arrives, the socket is closed, or the process is killed.
    pub fn receive(&self) -> Result<(SockAddr, Box<[u8]>), NetError> {
        let deadline = match self.inner.lock().timeout {
            0 => None,
            timeout => Some(TICKS.lock().saturating_add(timeout)),
        };

        loop {
            if proc::current_proc().is_killed() {
                err!(NetError::Interrupted)
            }

            let mut inner = self.inner.lock();
            if inner.closed {
                err!(NetError::BadSocket)
            }

            if let Some(entry) = inner.entries.pop_front() {
                return Ok(entry);
            }

            if deadline.is_some_and(|d| d < *TICKS.lock()) {
                err!(NetError::TimedOut)
            }

            inner = proc::sleep_until(self.channel(), deadline, inner);
        }
    }

    pub fn close(&self) {
        self.inner.lock().closed = true;

        proc::wakeup(self.channel());
    }

    pub fn is_closed(&self) -> bool {
        self.inner.lock().closed
    }

    fn channel(&self) -> Channel {
        Channel::Socket(self as *const _ as usize)
    }
}
