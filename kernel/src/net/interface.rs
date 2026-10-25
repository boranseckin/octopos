use core::sync::atomic::{AtomicU16, Ordering};

use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::net::loopback;
use crate::net::route;
use crate::net::{Ipv4Addr, Ipv4Cidr, MacAddr, NetError};
use crate::proc::Pod;
use crate::spinlock::SpinLock;

/// Trait for network interface devices
pub trait NetDevice: Send + Sync {
    fn transmit(&self, packet: &[u8]) -> Result<(), NetError>;
    fn needs_arp(&self) -> bool {
        true
    }
    fn needs_dhcp(&self) -> bool {
        true
    }
}

/// Identifier for a network interface, used in routing table entries to specify which interface to
/// send packets on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceId(pub u16);

impl InterfaceId {
    pub fn next_interface_id() -> Self {
        static ID_COUNTER: AtomicU16 = AtomicU16::new(0);
        Self(ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

impl core::fmt::Display for InterfaceId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "if{}", self.0)
    }
}

/// Configuration for a network interface.
#[derive(Debug, Clone)]
pub struct InterfaceConfig {
    /// Name
    pub name: &'static str,
    /// MAC address
    pub mac: MacAddr,
    /// IPv4 configuration, if configured.
    /// If `None`, the interface is not configured and cannot send or receive IP packets.
    pub ipv4: Option<Ipv4Cidr>,
    /// Whether the interface is functional or not.
    pub is_up: bool,
}

/// Interface information for routing and packet transmission.
#[derive(Clone)]
pub struct InterfaceEntry {
    /// Identifier
    pub id: InterfaceId,
    /// Configuration used for routing.
    pub config: InterfaceConfig,
    /// Reference to the underlying network device, used for sending and receiving packets.
    pub device: Arc<dyn NetDevice>,
}

pub struct InterfaceFlag;

impl InterfaceFlag {
    pub const UP: u16 = 1 << 0;
    pub const LOOPBACK: u16 = 1 << 1;
    pub const CONFIGURED: u16 = 1 << 2;
}

/// Wire format of an interface shared with user space.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct RawInterface {
    pub id: u16,
    pub flags: u16,
    pub name: [u8; 16],
    pub mac: [u8; 6],
    pub prefix_len: u8,
    pub _pad: u8,
    pub addr: [u8; 4],
}

/// # Safety: only contains primitive types, padded to align to 2 bytes
unsafe impl Pod for RawInterface {}

impl From<InterfaceEntry> for RawInterface {
    fn from(value: InterfaceEntry) -> Self {
        let config = value.config;

        let mut flags = 0;
        if config.is_up {
            flags |= InterfaceFlag::UP;
        }
        if value.id == loopback::interface_id() {
            flags |= InterfaceFlag::LOOPBACK;
        }
        if config.ipv4.is_some() {
            flags |= InterfaceFlag::CONFIGURED;
        }

        // NULL-padded, truncated to fit
        let mut name = [0u8; 16];
        let len = config.name.len().min(name.len());
        name[..len].copy_from_slice(&config.name.as_bytes()[..len]);

        let (addr, prefix_len) = config
            .ipv4
            .map_or(([0; 4], 0), |ipv4| (ipv4.addr.0, ipv4.prefix_len));

        Self {
            id: value.id.0,
            flags,
            name,
            mac: config.mac.0,
            prefix_len,
            _pad: 0u8,
            addr,
        }
    }
}

static INTERFACES: SpinLock<Vec<InterfaceEntry>> = SpinLock::new(Vec::new(), "net_interfaces");

/// Returns a copy of the interface entry at a given index.
pub fn interface_at(index: usize) -> Result<InterfaceEntry, NetError> {
    INTERFACES
        .lock()
        .get(index)
        .ok_or(NetError::InterfaceNotFound)
        .cloned()
}

/// Registers a network interface with the given id and the network stack.
pub fn register_interface(id: InterfaceId, config: InterfaceConfig, device: Arc<dyn NetDevice>) {
    if let Some(ipv4) = config.ipv4 {
        route::replace_interface_route(id, ipv4);
    };

    INTERFACES
        .lock()
        .push(InterfaceEntry { id, config, device });
}

/// Sets the IPv4 configuration for a network interface, replacing any existing configuration.
///
/// If `ipv4` is `None`, the interface is marked as not configured and cannot send or receive IP
/// packets.
pub fn set_interface_ipv4(id: InterfaceId, ipv4: Option<Ipv4Cidr>) -> Result<(), NetError> {
    // if address is being set, make sure it has a valid prefix len.
    // /0 would make every destination on-link; not allowed.
    if let Some(c) = ipv4
        && (c.prefix_len == 0 || Ipv4Addr::prefix_len_to_mask(c.prefix_len).is_none())
    {
        err!(NetError::InvalidAddress)
    }

    let mut ifs = INTERFACES.lock();

    let Some(interface) = ifs.iter_mut().find(|i| i.id == id) else {
        err!(NetError::InterfaceNotFound)
    };

    interface.config.ipv4 = ipv4;

    // drop interfaces lock before add_route
    drop(ifs);

    // replace interfaces ipv4 routes or remove all routes.
    if let Some(ipv4) = ipv4 {
        route::replace_interface_route(id, ipv4);
    } else {
        route::remove_interface_routes(id);
    }

    Ok(())
}

/// Finds a network interface by its identifier.
///
/// Returned entry is a clone of the one in the registry. This value could be stale immediately
/// after it's returned.
pub fn find_interface_by_id(id: InterfaceId) -> Option<InterfaceEntry> {
    let ifs = INTERFACES.lock();
    ifs.iter().find(|i| i.id == id).cloned()
}

/// Prints the interface registry for debugging purposes.
pub fn dump() {
    let ifs = INTERFACES.lock();

    println!("");
    for interface in ifs.iter() {
        println!(
            "{}: {} {} {} {}",
            interface.id,
            interface.config.name,
            interface.config.mac,
            interface
                .config
                .ipv4
                .map(|ip| format!("{}/{}", ip.addr, ip.prefix_len))
                .unwrap_or_else(|| "unconfigured".to_string()),
            if interface.config.is_up { "up" } else { "down" }
        );
    }
}
