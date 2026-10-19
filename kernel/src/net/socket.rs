use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;

use crate::net::icmp::PingSocket;
use crate::net::udp::UdpSocket;
use crate::net::{Ipv4Addr, NetError};
use crate::proc::{self, Channel, Pod};
use crate::spinlock::SpinLock;

const MAX_RECV_QUEUE_DEPTH: usize = 8;

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketType {
    Udp = 1,
    Ping = 2,
}

impl TryFrom<usize> for SocketType {
    type Error = NetError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Udp),
            2 => Ok(Self::Ping),
            _ => Err(NetError::BadSocket),
        }
    }
}

/// Address of a socket endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SockAddr {
    /// IPv4 address and port, used by UDP sockets.
    Inet { ip: Ipv4Addr, port: u16 },
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
}

/// Wire format of a socket address shared with user space.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct RawSockAddr {
    pub family: u16,
    pub port: u16,
    pub addr: [u8; 4],
}

// Safety: RawSockAddr is repr(C), with no padding, and with primitive types.
unsafe impl Pod for RawSockAddr {}

impl From<SockAddr> for RawSockAddr {
    fn from(value: SockAddr) -> Self {
        match value {
            SockAddr::Inet { ip, port } => Self {
                family: 1,
                port,
                addr: ip.0,
            },
        }
    }
}

impl TryFrom<RawSockAddr> for SockAddr {
    type Error = NetError;

    fn try_from(value: RawSockAddr) -> Result<Self, Self::Error> {
        match value.family {
            1 => Ok(Self::Inet {
                ip: Ipv4Addr::from(value.addr),
                port: value.port,
            }),
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
}

impl Socket {
    pub fn open_udp(ip: Ipv4Addr, port: u16) -> Result<Self, NetError> {
        Ok(Self::Udp(try_log!(UdpSocket::bind(ip, port, None))))
    }

    pub fn open_ping(ident: u16) -> Result<Self, NetError> {
        Ok(Self::Ping(try_log!(PingSocket::bind(ident))))
    }

    pub fn close(&self) {
        match self {
            Socket::Udp(udp_socket) => udp_socket.close(),
            Socket::Ping(ping_socket) => ping_socket.close(),
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
        }
    }

    pub fn send(&self, remote_addr: SockAddr, buf: &[u8]) -> Result<(), NetError> {
        match self {
            Socket::Udp(udp_socket) => {
                let SockAddr::Inet { ip, port } = remote_addr;
                udp_socket.send(ip, port, buf)
            }

            Socket::Ping(ping_socket) => {
                let SockAddr::Inet { ip, port: _ } = remote_addr;
                ping_socket.send(ip, buf)
            }
        }
    }

    pub fn receive(&self) -> Result<(SockAddr, Box<[u8]>), NetError> {
        match self {
            Socket::Udp(udp_socket) => {
                let (ip, port, buf) = try_log!(udp_socket.receive());
                Ok((SockAddr::Inet { ip, port }, buf))
            }

            Socket::Ping(ping_socket) => log!(ping_socket.receive()),
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
}

impl RecvQueue {
    /// Creates a new RecvQueue with capacity `MAX_RECV_QUEUE_DEPTH`.
    pub fn new() -> Self {
        Self {
            inner: SpinLock::new(
                RecvQueueInner {
                    entries: VecDeque::with_capacity(MAX_RECV_QUEUE_DEPTH),
                    closed: false,
                },
                "recv_queue",
            ),
        }
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

            inner = proc::sleep(self.channel(), inner);
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
