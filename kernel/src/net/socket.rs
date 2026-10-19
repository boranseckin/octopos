use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::net::udp::UdpSocket;
use crate::net::{Ipv4Addr, NetError};

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
