use alloc::vec;

use crate::abi::{RawSockAddr, SocketType};
use crate::file::{FILE_TABLE, File, FileType};
use crate::net::socket::{SockAddr, Socket};
use crate::net::udp::MAX_UDP_PAYLOAD;
use crate::proc::{self, copy_from_user, copy_struct_from_user};
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

/// Opens a new UDP socket and returns a file descriptor for it.
///
/// # Arguments
///
/// - `a0` (`SocketType`): type of socket.
/// - `a1` (`*const RawSockAddr`): address and port of socket. 0.0.0.0:0 is wildcard IP, ephemeral
///   port/ident.
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

    match socket_type {
        SocketType::Udp => {
            let raw_addr_va = args.get_addr(1);
            let raw_addr: RawSockAddr =
                try_log!(copy_struct_from_user(raw_addr_va).map_err(|_| SysError::BadAddress));
            let addr = try_log!(SockAddr::try_from(raw_addr).map_err(SysError::from));

            let SockAddr::Inet { ip, port } = addr;
            let socket = try_log!(Socket::open_udp(ip, port).map_err(SysError::from));

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

        SocketType::Ping => todo!(),
    }
}

/// Sends a UDP datagram to the specified destination.
///
/// # Arguments
///
/// - `a0` (`Fd`): socket file descriptor.
/// - `a1` (`*const u8`): pointer to the payload buffer.
/// - `a2` (`usize`): payload length in bytes. Must not exceed `MAX_UDP_PAYLOAD`.
/// - `a3` (`*const RawSockAddr`): pointer to the destination socket address.
///
/// # Returns
///
/// `Ok(n)`: number of bytes sent, equal to the payload length.
///
/// # Errors
///
/// - `MessageTooLarge`: payload length exceeds `MAX_UDP_PAYLOAD`.
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
    if buf_len > MAX_UDP_PAYLOAD {
        err!(SysError::MessageTooLarge)
    }

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

/// Receives a UDP datagram on the socket, blocking until one arrives.
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
