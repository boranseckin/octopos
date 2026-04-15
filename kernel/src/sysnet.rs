use alloc::vec;

use crate::file::{FILE_TABLE, File, FileType};
use crate::net::Ipv4Addr;
use crate::net::udp::{MAX_UDP_PAYLOAD, SocketTable};
use crate::proc::{self, copy_from_user};
use crate::syscall::{SysError, SyscallArgs};
use crate::sysfile::fd_alloc;

pub fn sys_socket(args: &SyscallArgs) -> Result<usize, SysError> {
    let Ok(port) = u16::try_from(args.get_int(0)) else {
        err!(SysError::InvalidArgument)
    };

    let socket_id = try_log!(SocketTable::open(port).map_err(SysError::from));

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

pub fn sys_send(args: &SyscallArgs) -> Result<usize, SysError> {
    let buf_addr = args.get_addr(1);
    let buf_len = args.get_int(2) as usize;
    let dest_ip_ptr = args.get_addr(3);
    let Ok(dest_port) = u16::try_from(args.get_int(4)) else {
        err!(SysError::InvalidArgument)
    };
    let (_, file) = try_log!(args.get_file(0));

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

    // clamp payload to prevent malicious heap allocation
    let payload_len = buf_len.min(MAX_UDP_PAYLOAD);
    let mut payload = vec![0u8; payload_len];
    if log!(copy_from_user(buf_addr, &mut payload)).is_err() {
        err!(SysError::BadAddress)
    }

    try_log!(SocketTable::send(socket_id, dest_ip, dest_port, &payload));

    Ok(payload_len)
}

pub fn sys_receive(args: &SyscallArgs) -> Result<usize, SysError> {
    let buf_addr = args.get_addr(1);
    let buf_len = args.get_int(2) as usize;
    let src_ip_ptr = args.get_addr(3);
    let src_port_ptr = args.get_addr(4);
    let (_, file) = try_log!(args.get_file(0));

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
