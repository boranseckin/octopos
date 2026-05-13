use core::sync::atomic::{AtomicU16, Ordering};

use alloc::format;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::net::dhcp;
use crate::net::route::{self, RouteEntry, RouteOwner};
use crate::net::{Ipv4Addr, Ipv4Config, MacAddr, NetError};
use crate::proc;
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
    pub ipv4: Option<Ipv4Config>,
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

static INTERFACES: SpinLock<Vec<InterfaceEntry>> = SpinLock::new(Vec::new(), "net_interfaces");

/// Registers a network interface with the network stack and returns its assigned `InterfaceId.
pub fn register_interface(config: InterfaceConfig, device: Arc<dyn NetDevice>) -> InterfaceId {
    let id = InterfaceId::next_interface_id();

    if let Some(ipv4) = config.ipv4 {
        route::upsert_route(RouteEntry {
            dest_ip: ipv4.addr
                & Ipv4Addr::prefix_len_to_mask(ipv4.prefix_len).expect("valid prefix length"),
            prefix_len: ipv4.prefix_len,
            gateway: None,
            interface_id: id,
            metric: 10,
            owner: RouteOwner::Interface,
        });
    };

    let needs_dhcp = device.needs_dhcp();
    let name = config.name;

    INTERFACES
        .lock()
        .push(InterfaceEntry { id, config, device });

    if needs_dhcp {
        let thread_name = format!("dhcp-{}", name);
        proc::spawn_kernel_thread(move || dhcp::dhcp_thread(id), &thread_name);
    }

    id
}

/// Sets the IPv4 configuration for a network interface, replacing any existing configuration.
///
/// If `ipv4` is `None`, the interface is marked as not configured and cannot send or receive IP
/// packets.
pub fn set_interface_ipv4(id: InterfaceId, ipv4: Option<Ipv4Config>) -> Result<(), NetError> {
    let mut ifs = INTERFACES.lock();

    let Some(interface) = ifs.iter_mut().find(|i| i.id == id) else {
        err!(NetError::InterfaceNotFound)
    };

    interface.config.ipv4 = ipv4;

    // drop interfaces lock before add_route
    drop(ifs);

    if let Some(ipv4) = ipv4 {
        route::replace_interface_route(id, ipv4);
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
