use alloc::vec::Vec;

use kernel::abi::{RawInterface, RawRoute, RawSockAddr, SysError};

use crate::io::Fd;
use crate::syscall::{check, check_unit, raw};

pub use kernel::abi::{
    Be, EtherType, EthernetHeader, InterfaceFlag, InterfaceId, Ipv4Addr, Ipv4Cidr, Ipv4Header,
    Ipv4Proto, MacAddr, NetworkHeader, RouteEntry, RouteFlag, RouteKind, RouteOwner, SockAddr,
    SocketType, UdpHeader,
};

pub fn socket(typ: SocketType, addr: SockAddr) -> Result<Fd, SysError> {
    check(raw::socket(
        typ as usize,
        &RawSockAddr::from(addr) as *const RawSockAddr,
    ))
    .map(Fd)
}

pub fn send(fd: Fd, buf: &[u8], dest_addr: SockAddr) -> Result<usize, SysError> {
    check(raw::send(
        fd.as_raw(),
        buf.as_ptr(),
        buf.len(),
        &RawSockAddr::from(dest_addr) as *const RawSockAddr,
    ))
}

/// A datagram received into the caller's buffer.
pub struct Received<'a> {
    /// The bytes written into the buffer.
    pub data: &'a [u8],
    /// Source address.
    pub src: SockAddr,
    /// Indicator that the datagram was larger than the buffer, and the rest was discarded.
    pub truncated: bool,
}

impl Received<'_> {
    /// Bytes written into the buffer.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub fn receive(fd: Fd, buf: &mut [u8]) -> Result<Received<'_>, SysError> {
    let mut src_addr = RawSockAddr::default();

    let full_len = check(raw::receive(
        fd.as_raw(),
        buf.as_mut_ptr(),
        buf.len(),
        &mut src_addr as *mut RawSockAddr,
    ))?;

    let len = full_len.min(buf.len());

    Ok(Received {
        data: &buf[..len],
        src: SockAddr::try_from(src_addr).expect("kernel to send correct RawSockAddr"),
        truncated: full_len > len,
    })
}

pub fn interface_get(index: usize) -> Result<RawInterface, SysError> {
    let mut raw_interface = RawInterface::default();

    check_unit(raw::interface_get(
        &mut raw_interface as *mut RawInterface,
        index,
    ))?;

    Ok(raw_interface)
}

/// Sets the IPv4 address of interface `id`. `0.0.0.0` clears it.
pub fn interface_set_ipv4(id: InterfaceId, addr: Ipv4Addr, prefix_len: u8) -> Result<(), SysError> {
    let raw_interface = RawInterface {
        id: id.0,
        addr: addr.0,
        prefix_len,
        ..Default::default()
    };

    check_unit(raw::interface_set_ipv4(
        &raw_interface as *const RawInterface,
    ))
}

pub fn route_get(index: usize) -> Result<RouteEntry, SysError> {
    let mut raw_route = RawRoute::default();

    check(raw::route_get(&mut raw_route as *mut RawRoute, index))?;

    Ok(RouteEntry::try_from(raw_route)?)
}

pub fn route_add(route: &RouteEntry, flags: usize) -> Result<(), SysError> {
    let raw_route = RawRoute::from(*route);

    check_unit(raw::route_add(&raw_route as *const RawRoute, flags))
}

pub fn route_remove(route: &RouteEntry) -> Result<(), SysError> {
    let raw_route = RawRoute::from(*route);

    check_unit(raw::route_remove(&raw_route as *const RawRoute))
}

#[derive(Debug, Clone, Copy)]
pub struct Interface {
    pub id: InterfaceId,
    pub mac: MacAddr,
    pub ipv4: Option<Ipv4Cidr>,
    name: [u8; 16],
    flags: u16,
}

impl Interface {
    /// Returns the name up to the first NULL byte, or "?" if it is not valid UTF-8.
    pub fn name(&self) -> &str {
        let len = self
            .name
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(self.name.len());
        str::from_utf8(&self.name[..len]).unwrap_or("?")
    }

    pub fn is_up(&self) -> bool {
        (self.flags & InterfaceFlag::UP) != 0
    }

    pub fn is_loopback(&self) -> bool {
        (self.flags & InterfaceFlag::LOOPBACK) != 0
    }

    pub fn is_configured(&self) -> bool {
        (self.flags & InterfaceFlag::CONFIGURED) != 0
    }
}

impl From<RawInterface> for Interface {
    fn from(value: RawInterface) -> Self {
        let ipv4 = ((value.flags & InterfaceFlag::CONFIGURED) != 0).then_some(Ipv4Cidr {
            addr: Ipv4Addr(value.addr),
            prefix_len: value.prefix_len,
        });

        Interface {
            id: InterfaceId(value.id),
            mac: MacAddr(value.mac),
            ipv4,
            name: value.name,
            flags: value.flags,
        }
    }
}

/// Returns an iterator over every interface, in kernel order.
/// Stops after `NoEntry`; yields other errors once, then stops.
///
/// Iterator fetches interfaces lazily, so the underlying data can change between calls to `next()`.
/// If you need a static snapshot, use `interfaces()` instead.
pub fn interface_iter() -> impl Iterator<Item = Result<Interface, SysError>> {
    let mut index = 0;
    let mut done = false;

    core::iter::from_fn(move || {
        if done {
            return None;
        }

        match interface_get(index) {
            Ok(raw) => {
                index += 1;
                Some(Ok(Interface::from(raw)))
            }

            Err(SysError::NoEntry) => {
                done = true;
                None
            }

            Err(e) => {
                done = true;
                Some(Err(e))
            }
        }
    })
}

/// Returns every interface at once, in kernel order.
pub fn interfaces() -> Result<Vec<Interface>, SysError> {
    interface_iter().collect()
}

/// Finds an interface with the given `id`.
pub fn interface_by_id(id: InterfaceId) -> Result<Option<Interface>, SysError> {
    interface_iter()
        // map_or(true, ..) counts Err as found
        .find(|r| r.as_ref().map_or(true, |i| i.id == id))
        .transpose()
}

/// Finds an interface with the given `name`.
pub fn interface_by_name(name: &str) -> Result<Option<Interface>, SysError> {
    interface_iter()
        // map_or(true, ..) counts Err as found
        .find(|r| r.as_ref().map_or(true, |i| i.name() == name))
        .transpose()
}

/// Returns the first loopback interface.
pub fn loopback() -> Result<Option<Interface>, SysError> {
    interface_iter()
        // map_or(true, ..) counts Err as found
        .find(|r| r.as_ref().map_or(true, |i| i.is_loopback()))
        .transpose()
}

/// Resolves the interface with the given name or numeric id.
/// A number that names an existing interface wins over a name lookup.
pub fn resolve_interface(dev: &str) -> Result<Option<Interface>, SysError> {
    if let Ok(id) = dev.parse()
        && let Some(i) = interface_by_id(InterfaceId(id))?
    {
        return Ok(Some(i));
    }

    interface_by_name(dev)
}
