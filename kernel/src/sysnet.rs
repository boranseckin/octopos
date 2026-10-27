use alloc::vec;

use crate::file::{FILE_TABLE, File, FileType};
use crate::net::interface::{self, InterfaceId, RawInterface};
use crate::net::route::{self, RawRoute, RouteEntry};
use crate::net::socket::{RawSockAddr, SockAddr, Socket, SocketType};
use crate::net::{Ipv4Addr, Ipv4Cidr};
use crate::proc::{self, copy_from_user, copy_struct_from_user, copy_struct_to_user};
use crate::syscall::{SysError, SyscallArgs};
use crate::sysfile::fd_alloc;

/// Returns the socket behind `file`, or `InvalidArgument` if it is not a socket.
/// The file lock is released on return, so callers may block on the socket.
fn file_socket(file: &File) -> Result<Socket, SysError> {
    let inner = FILE_TABLE.inner[file.id].lock();
    let FileType::Socket { socket } = &inner.r#type else {
        err!(SysError::InvalidArgument)
    };
    Ok(socket.clone())
}

/// Opens a new socket and returns a file descriptor for it.
///
/// # Arguments
///
/// - `a0` (`SocketType`): type of socket.
/// - `a1` (`*const RawSockAddr`): address of socket.
///
/// # Returns
///
/// `Ok(fd)`: file descriptor for the newly opened socket.
///
/// # Errors
///
/// - `InvalidArgument`: the socket type or the address family is wrong.
/// - `AlreadyExists`: the requested port is already bound by another socket.
/// - `ResourceUnavailable`: no free ephemeral ports (only when `a1 == null`).
/// - `FileTableFull`: the system-wide file or socket table is full.
/// - `TooManyFiles`: the calling process has no free file descriptor slots.
pub fn sys_socket(args: &SyscallArgs) -> Result<usize, SysError> {
    let Ok(socket_type) = usize::try_from(args.get_int(0)) else {
        err!(SysError::InvalidArgument)
    };

    let Ok(socket_type) = SocketType::try_from(socket_type) else {
        err!(SysError::InvalidArgument)
    };

    let raw_addr_va = args.get_addr(1);
    let raw_addr: RawSockAddr =
        try_log!(copy_struct_from_user(raw_addr_va).map_err(|_| SysError::BadAddress));
    let addr = try_log!(SockAddr::try_from(raw_addr).map_err(SysError::from));

    let socket = match socket_type {
        SocketType::Udp => {
            let SockAddr::Inet { ip, port } = addr else {
                err!(SysError::InvalidArgument);
            };
            try_log!(Socket::open_udp(ip, port).map_err(SysError::from))
        }

        SocketType::Ping => {
            let SockAddr::Inet { ip: _, port } = addr else {
                err!(SysError::InvalidArgument);
            };
            try_log!(Socket::open_ping(port).map_err(SysError::from))
        }

        SocketType::Packet => {
            let SockAddr::Link {
                interface,
                ether_type,
                mac: _,
            } = addr
            else {
                err!(SysError::InvalidArgument)
            };
            try_log!(Socket::open_packet(interface, ether_type).map_err(SysError::from))
        }
    };

    // allocate a file structure and a file descriptor
    let (fd, file) = match log!(File::alloc()) {
        Ok(mut file) => match log!(fd_alloc(file.clone())) {
            Ok(fd) => (fd, file),
            Err(e) => {
                file.close();
                socket.close();
                return Err(e);
            }
        },
        Err(e) => {
            socket.close();
            return Err(SysError::from(e));
        }
    };

    // set file type; writeable/readable stays as false.
    let mut inner = FILE_TABLE.inner[file.id].lock();
    inner.r#type = FileType::Socket { socket };

    Ok(fd)
}

/// Sends a packet on the socket.
///
/// # Arguments
///
/// - `a0` (`Fd`): socket file descriptor.
/// - `a1` (`*const u8`): pointer to the payload buffer.
/// - `a2` (`usize`): payload length in bytes.
/// - `a3` (`*const RawSockAddr`): pointer to the destination socket address.
///
/// # Returns
///
/// `Ok(n)`: number of bytes sent, equal to the payload length.
///
/// # Errors
///
/// - `MessageTooLarge`: payload length exceeds limit.
/// - `InvalidArgument`: `a0` is not a socket file descriptor, or `a4` does not fit in a `u16`.
/// - `BadDescriptor`: `a0` is not a valid open file descriptor.
/// - `BadAddress`: `a1` or `a3` is not a valid user-space pointer.
/// - `NotPermitted`: no network interface is configured.
/// - `NoEntry`: no route to the destination address.
/// - `ResourceUnavailable`: the outgoing packet queue or the transmit ring is full.
/// - `IoError`: the network interface failed to transmit the packet.
pub fn sys_send(args: &SyscallArgs) -> Result<usize, SysError> {
    let (_, file) = try_log!(args.get_file(0));
    let buf_addr = args.get_addr(1);
    let buf_len = args.get_int(2) as usize;

    let socket = try_log!(file_socket(&file));

    let raw_addr_va = args.get_addr(3);
    let raw_addr: RawSockAddr =
        try_log!(copy_struct_from_user(raw_addr_va).map_err(|_| SysError::BadAddress));
    let addr = try_log!(SockAddr::try_from(raw_addr).map_err(SysError::from));

    let mut payload = vec![0u8; buf_len];
    if log!(copy_from_user(buf_addr, &mut payload)).is_err() {
        err!(SysError::BadAddress)
    }

    try_log!(socket.send(addr, &payload));

    Ok(buf_len)
}

/// Receives a packet from the socket, blocking until one arrives.
///
/// # Arguments
///
/// - `a0` (`Fd`): socket file descriptor.
/// - `a1` (`*mut u8`): pointer to the receive buffer.
/// - `a2` (`usize`): receive buffer capacity in bytes. If the datagram is larger, excess bytes
///   are silently discarded.
/// - `a3` (`*mut RawSockAddr`): pointer to a source socket address.
///
/// # Returns
///
/// `Ok(n)`: number of bytes written into the receive buffer.
///
/// # Errors
///
/// - `InvalidArgument`: `a0` is not a socket file descriptor.
/// - `BadDescriptor`: `a0` is not a valid open file descriptor, or the socket was closed while
///   waiting for a datagram.
/// - `BadAddress`: one of the output pointers (`a1`, `a3`, `a4`) is not a valid user-space
///   pointer.
/// - `Interrupted`: the calling process was killed while waiting for a datagram.
pub fn sys_receive(args: &SyscallArgs) -> Result<usize, SysError> {
    let (_, file) = try_log!(args.get_file(0));
    let buf_addr = args.get_addr(1);
    let buf_len = args.get_int(2) as usize;
    let src_addr_ptr = args.get_addr(3);

    let socket = try_log!(file_socket(&file));

    let (addr, received) = try_log!(socket.receive());
    let copy_len = received.len().min(buf_len);
    let raw_addr = RawSockAddr::from(addr);

    if log!(proc::copy_struct_to_user(&raw_addr, src_addr_ptr)).is_err()
        || log!(proc::copy_to_user(&received[..copy_len], buf_addr)).is_err()
    {
        err!(SysError::BadAddress)
    }

    Ok(copy_len)
}

#[repr(usize)]
pub enum InterfaceOp {
    Get = 1,
    SetIpv4 = 2,
}

impl TryFrom<usize> for InterfaceOp {
    type Error = SysError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Get),
            2 => Ok(Self::SetIpv4),
            _ => err!(SysError::InvalidArgument),
        }
    }
}

/// Reads or configures a network interface.
///
/// # Arguments
///
/// - `a0` (`InterfaceOp`): operation to perform.
/// - `a1` (`*mut RawInterface`): pointer to the interface struct. Its direction depends on the
///   operation.
/// - `a2` (`usize`): operation-specific argument.
///
/// # Operations
///
/// - `Get`: writes the interface at index `a2` into `a1`. Indices start at 0 and are contiguous;
///   iterate until `NoEntry`. The table may change between calls, so a listing can skip or
///   repeat an entry.
/// - `SetIpv4`: reads `id`, `addr` and `prefix_len` from `a1` and sets that interface's address.
///   The kernel replaces its own routes for the interface. An `addr` of 0.0.0.0 clears the address
///   and removes every route on the interface. The other fields of `a1` are ignored, as is `a2`.
pub fn sys_interface(args: &SyscallArgs) -> Result<usize, SysError> {
    let op = try_log!(InterfaceOp::try_from(args.get_int(0) as usize));
    let raw_interface_va = args.get_addr(1);

    match op {
        InterfaceOp::Get => {
            let index = args.get_int(2) as usize;

            let interface = try_log!(interface::interface_at(index));
            let raw_interface = RawInterface::from(interface);

            try_log!(
                copy_struct_to_user(&raw_interface, raw_interface_va)
                    .map_err(|_| SysError::BadAddress)
            );
        }

        InterfaceOp::SetIpv4 => {
            let raw_interface: RawInterface =
                try_log!(copy_struct_from_user(raw_interface_va).map_err(|_| SysError::BadAddress));

            let addr = Ipv4Addr::from(raw_interface.addr);
            let ipv4 = (addr != Ipv4Addr::UNSPECIFIED).then_some(Ipv4Cidr {
                addr,
                prefix_len: raw_interface.prefix_len,
            });

            try_log!(interface::set_interface_ipv4(
                InterfaceId(raw_interface.id),
                ipv4
            ))
        }
    }

    Ok(0)
}

#[repr(usize)]
pub enum RouteOp {
    Get = 1,
    Add = 2,
    Remove = 3,
}

impl TryFrom<usize> for RouteOp {
    type Error = SysError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Get),
            2 => Ok(Self::Add),
            3 => Ok(Self::Remove),
            _ => err!(SysError::InvalidArgument),
        }
    }
}

pub struct RouteFlag;

impl RouteFlag {
    pub const ROUTE_REPLACE: usize = 1;
}

/// Reads or modifies the routing table.
///
/// # Arguments
///
/// - `a0` (`RouteOp`): operation to perform.
/// - `a1` (`*mut RawRoute`): pointer to the route struct. Its direction depends on the operation.
/// - `a2` (`usize`): operation-specific argument.
///
/// # Operations
///
/// - `Get`: writes the route at index `a2` into `a1`. Indices start at 0 and are contiguous;
///   iterate until `NoEntry`. The table may change between calls, so a listing can skip or
///   repeat an entry.
/// - `Add`: reads a route from `a1` and adds it. `a2` holds `RouteFlag` bits. With
///   `ROUTE_REPLACE`, an existing route with the same key is overwritten instead of rejected.
/// - `Remove`: reads a route from `a1` and removes the stored route with the same key. Fields
///   outside the key are ignored. `a2` is ignored.
///
/// A route's key is its destination, prefix length, gateway and interface. In `RawRoute`, a
/// `gateway` or `pref_src` of 0.0.0.0 means `None`.
///
/// Only routes owned by `Dhcp` or `Static` can be added or removed. `Kernel` routes and `Local`
/// routes are created and removed by the kernel when interface addresses change.
pub fn sys_route(args: &SyscallArgs) -> Result<usize, SysError> {
    let op = try_log!(RouteOp::try_from(args.get_int(0) as usize));
    let raw_route_va = args.get_addr(1);

    match op {
        RouteOp::Get => {
            let index = args.get_int(2) as usize;

            let Some(route) = route::route_at(index) else {
                err!(SysError::NoEntry)
            };
            let raw_route = RawRoute::from(route);

            try_log!(
                copy_struct_to_user(&raw_route, raw_route_va).map_err(|_| SysError::BadAddress)
            );

            Ok(0)
        }

        RouteOp::Add => {
            let flags = args.get_int(2) as usize;
            let replace = flags & RouteFlag::ROUTE_REPLACE != 0;

            let raw_route: RawRoute =
                try_log!(copy_struct_from_user(raw_route_va).map_err(|_| SysError::BadAddress));
            let route = try_log!(RouteEntry::try_from(raw_route));

            try_log!(route::add_route(route, replace));

            Ok(0)
        }

        RouteOp::Remove => {
            let raw_route: RawRoute =
                try_log!(copy_struct_from_user(raw_route_va).map_err(|_| SysError::BadAddress));
            let route = try_log!(RouteEntry::try_from(raw_route));

            try_log!(route::remove_route(&route));

            Ok(0)
        }
    }
}
