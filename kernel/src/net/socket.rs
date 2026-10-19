use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;

use crate::net::udp::UdpSocket;
use crate::net::{Ipv4Addr, NetError};
use crate::proc::{self, Channel};
use crate::spinlock::SpinLock;

const MAX_RECV_QUEUE_DEPTH: usize = 8;

/// Address of a socket endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SockAddr {
    /// IPv4 address and port, used by UDP sockets.
    Inet { ip: Ipv4Addr, port: u16 },
}

/// Handle to a socket of any type. Cloning refers to the same underlying socket.
/// The socket stays bound until `close()` is called, regardles of how many handles exists.
#[derive(Debug, Clone)]
pub enum Socket {
    Udp(Arc<UdpSocket>),
}

impl Socket {
    pub fn open_udp(ip: Ipv4Addr, port: u16) -> Result<Self, NetError> {
        Ok(Self::Udp(try_log!(UdpSocket::bind(ip, port, None))))
    }

    pub fn close(&self) {
        match self {
            Socket::Udp(udp_socket) => udp_socket.close(),
        }
    }

    pub fn local_addr(&self) -> SockAddr {
        match self {
            Socket::Udp(udp_socket) => SockAddr::Inet {
                ip: udp_socket.bound_ip,
                port: udp_socket.bound_port,
            },
        }
    }

    pub fn send(&self, remote_addr: SockAddr, buf: &[u8]) -> Result<(), NetError> {
        match self {
            Socket::Udp(udp_socket) => {
                let SockAddr::Inet { ip, port } = remote_addr;
                udp_socket.send(ip, port, buf)
            }
        }
    }

    pub fn receive(&self) -> Result<(SockAddr, Box<[u8]>), NetError> {
        match self {
            Socket::Udp(udp_socket) => {
                let (ip, port, buf) = try_log!(udp_socket.receive());
                Ok((SockAddr::Inet { ip, port }, buf))
            }
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
