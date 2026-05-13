use alloc::vec;

use crate::file::{FILE_TABLE, File, FileType};
use crate::net::Ipv4Addr;
use crate::net::udp::{MAX_UDP_PAYLOAD, SocketTable};
use crate::proc::{self, copy_from_user};
use crate::syscall::{SysError, SyscallArgs};
use crate::sysfile::fd_alloc;

/// Opens a new UDP socket and returns a file descriptor for it.
///
/// # Arguments
///
/// - `a0` (`u16`): port number to bind. Pass `0` to auto-assign an ephemeral port in the
///   range `[49152, 65535)`.
///
/// # Returns
///
/// `Ok(fd)`: file descriptor for the newly opened socket.
///
/// # Errors
///
/// - `InvalidArgument`: port value does not fit in a `u16`.
/// - `AlreadyExists`: the requested port is already bound by another socket.
/// - `ResourceUnavailable`: no free ephemeral ports (only when `a0 == 0`).
/// - `FileTableFull`: the system-wide file or socket table is full.
/// - `TooManyFiles`: the calling process has no free file descriptor slots.
pub fn sys_socket(args: &SyscallArgs) -> Result<usize, SysError> {
    let Ok(port) = u16::try_from(args.get_int(0)) else {
        err!(SysError::InvalidArgument)
    };

    let socket_id =
        try_log!(SocketTable::open(Ipv4Addr::UNSPECIFIED, port, None).map_err(SysError::from));

    // allocate a file structure and a file descriptor
    let (fd, file) = match log!(File::alloc()) {
        Ok(mut file) => match log!(fd_alloc(file.clone())) {
            Ok(fd) => (fd, file),
            Err(e) => {
                file.close();
                SocketTable::close(socket_id);
                return Err(e);
            }
        },
        Err(e) => {
            SocketTable::close(socket_id);
            return Err(SysError::from(e));
        }
    };

    // set file type; writeable/readable stays as false.
    let mut inner = FILE_TABLE.inner[file.id].lock();
    inner.r#type = FileType::Socket { socket_id };

    Ok(fd)
}

/// Sends a UDP datagram to the specified destination.
///
/// # Arguments
///
/// - `a0` (`Fd`): socket file descriptor.
/// - `a1` (`*const u8`): pointer to the payload buffer.
/// - `a2` (`usize`): payload length in bytes. Must not exceed `MAX_UDP_PAYLOAD`.
/// - `a3` (`*const [u8; 4]`): pointer to the destination IPv4 address in network byte order.
/// - `a4` (`u16`): destination port number in host byte order.
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
    let dest_ip_ptr = args.get_addr(3);
    let Ok(dest_port) = u16::try_from(args.get_int(4)) else {
        err!(SysError::InvalidArgument)
    };

    // check file type and retrieve socket_id, but drop the lock before moving forward.
    let socket_id = {
        let inner = FILE_TABLE.inner[file.id].lock();
        let FileType::Socket { socket_id } = inner.r#type else {
            err!(SysError::InvalidArgument)
        };
        socket_id
    };

    // copy IP in the network order
    let mut dest_ip = [0u8; 4];
    if log!(copy_from_user(dest_ip_ptr, &mut dest_ip)).is_err() {
        err!(SysError::BadAddress)
    }
    let dest_ip = Ipv4Addr(dest_ip);

    let mut payload = vec![0u8; buf_len];
    if log!(copy_from_user(buf_addr, &mut payload)).is_err() {
        err!(SysError::BadAddress)
    }

    try_log!(SocketTable::send(socket_id, dest_ip, dest_port, &payload));

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
/// - `a3` (`*mut [u8; 4]`): pointer to a 4-byte output buffer for the source IPv4 address in
///   network byte order.
/// - `a4` (`*mut u16`): pointer to a `u16` output for the source port in host byte order.
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
    let src_ip_ptr = args.get_addr(3);
    let src_port_ptr = args.get_addr(4);

    // check file type and retrieve socket_id, but drop the lock before receive since it might sleep
    let socket_id = {
        let inner = FILE_TABLE.inner[file.id].lock();
        let FileType::Socket { socket_id } = inner.r#type else {
            err!(SysError::InvalidArgument)
        };
        socket_id
    };

    let (ip, port, received) = try_log!(SocketTable::receive(socket_id));
    let copy_len = received.len().min(buf_len);

    if log!(proc::copy_to_user(&ip.0, src_ip_ptr)).is_err()
        || log!(proc::copy_to_user(&port.to_ne_bytes(), src_port_ptr)).is_err()
        || log!(proc::copy_to_user(&received[..copy_len], buf_addr)).is_err()
    {
        err!(SysError::BadAddress)
    }

    Ok(copy_len)
}
