use alloc::vec::Vec;

use crate::{
    InterfaceFlag, InterfaceId, Ipv4Addr, Ipv4Cidr, MacAddr, RawInterface, SysError, interface_get,
};

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

/// Returns every iterator at once, in kernel order.
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
