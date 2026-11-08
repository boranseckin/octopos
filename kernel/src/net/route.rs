use core::cmp;

use alloc::string::ToString;
use alloc::vec::Vec;

use crate::net::interface::{self, InterfaceId};
use crate::net::loopback;
use crate::net::{self, Ipv4Addr, Ipv4Cidr, NetError, Result};
use crate::proc::Pod;
use crate::spinlock::SpinLock;

/// Owner of a route entry, used for tracking which component added the route.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteOwner {
    /// Route added by an interface's address and prefix.
    Kernel = 1,
    /// Route added by the DHCP client when it receives an IP address lease.
    Dhcp = 2,
    /// Route added by a static configuration.
    Static = 3,
}

impl RouteOwner {
    pub const fn as_str(&self) -> &'static str {
        match self {
            RouteOwner::Kernel => "kernel",
            RouteOwner::Dhcp => "dhcp",
            RouteOwner::Static => "static",
        }
    }
}

impl TryFrom<u8> for RouteOwner {
    type Error = NetError;

    fn try_from(value: u8) -> core::result::Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Kernel),
            2 => Ok(Self::Dhcp),
            3 => Ok(Self::Static),
            _ => Err(NetError::NotPermitted),
        }
    }
}

impl core::fmt::Display for RouteOwner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.pad(self.as_str())
    }
}

/// How a matching packet is delivered.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteKind {
    /// Sent out of `interface_id`, directly or via `gateway`.
    Unicast = 1,
    /// Addressed to this host; delivered through loopback.
    /// `interface_id` is the interface that owns the address.
    Local = 2,
}

impl RouteKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            RouteKind::Unicast => "unicast",
            RouteKind::Local => "local",
        }
    }
}

impl TryFrom<u8> for RouteKind {
    type Error = NetError;

    fn try_from(value: u8) -> core::result::Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Unicast),
            2 => Ok(Self::Local),
            _ => Err(NetError::NotPermitted),
        }
    }
}

impl core::fmt::Display for RouteKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.pad(self.as_str())
    }
}

/// Routing information for a destination subnet.
#[derive(Debug, Clone, Copy)]
pub struct RouteEntry {
    /// Destination IPv4 address for this route.
    pub dest_ip: Ipv4Addr,
    /// Number of leading bits in the subnet mask for this route.
    pub prefix_len: u8,
    /// Next hop IP address for this route. If `None`, the destination is directly reachable on the
    /// link layer.
    pub gateway: Option<Ipv4Addr>,
    /// Interface the route belongs to.
    pub interface_id: InterfaceId,
    /// Routing metric for this route, used to determine the best route when multiple routes match a
    /// destination. Lower values indicate more preferred routes.
    pub metric: u32,
    /// Owner of this route, used for tracking which component added this route and allowing it to
    /// remove the route later if needed.
    pub owner: RouteOwner,
    /// Delivery kind of this route.
    pub kind: RouteKind,
    /// Preferred source address for packets using this route. If `None`, the outgoing interface's
    /// address is used.
    /// This is needed for local routes so that packets leave with their real IPs instead of
    /// 127.0.0.1, and replies come from the correct address.
    pub pref_src: Option<Ipv4Addr>,
}

/// Wire format of a route shared with user space.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct RawRoute {
    pub dest: [u8; 4],
    pub gateway: [u8; 4],  // 0.0.0.0 = direct
    pub pref_src: [u8; 4], // 0.0.0.0 = none
    pub metric: u32,
    pub interface_id: u16,
    pub owner: u8,
    pub kind: u8,
    pub prefix_len: u8,
    pub _pad: [u8; 3],
}

/// # Safety: only contains primitive type, padded to align to 4 bytes
unsafe impl Pod for RawRoute {}

impl From<RouteEntry> for RawRoute {
    fn from(value: RouteEntry) -> Self {
        Self {
            dest: value.dest_ip.0,
            gateway: value.gateway.map_or([0; 4], |g| g.0),
            pref_src: value.pref_src.map_or([0; 4], |g| g.0),
            metric: value.metric,
            interface_id: value.interface_id.0,
            owner: value.owner as u8,
            kind: value.kind as u8,
            prefix_len: value.prefix_len,
            _pad: [0; 3],
        }
    }
}

impl TryFrom<RawRoute> for RouteEntry {
    type Error = NetError;

    fn try_from(value: RawRoute) -> core::result::Result<Self, Self::Error> {
        Ok(Self {
            dest_ip: Ipv4Addr::from(value.dest),
            prefix_len: value.prefix_len,
            gateway: if value.gateway == [0; 4] {
                None
            } else {
                Some(Ipv4Addr::from(value.gateway))
            },
            interface_id: InterfaceId(value.interface_id),
            metric: value.metric,
            owner: RouteOwner::try_from(value.owner)?,
            kind: RouteKind::try_from(value.kind)?,
            pref_src: if value.pref_src == [0; 4] {
                None
            } else {
                Some(Ipv4Addr::from(value.pref_src))
            },
        })
    }
}

impl PartialEq for RouteEntry {
    fn eq(&self, other: &Self) -> bool {
        self.dest_ip == other.dest_ip
            && self.prefix_len == other.prefix_len
            && self.gateway == other.gateway
            && self.interface_id == other.interface_id
    }
}

#[derive(Debug)]
struct RouteTable {
    entries: Vec<RouteEntry>,
}

static ROUTES: SpinLock<RouteTable> = SpinLock::new(
    RouteTable {
        entries: Vec::new(),
    },
    "net_routes",
);

impl RouteTable {
    /// Returns a copy of the route entry at a given index in lookup order (best is first).
    fn get(&self, index: usize) -> Option<RouteEntry> {
        self.entries.get(index).copied()
    }

    /// Returns the index of the route in the table.
    fn position(&self, route: &RouteEntry) -> Option<usize> {
        self.entries.iter().position(|r| route.eq(r))
    }

    /// Insert the `route` while keeping the priority order according to `route_priority`.
    /// Ties keep insertion order: an older route with the same priority stays ahead.
    fn insert(&mut self, route: RouteEntry) {
        let key = route_priority(&route);
        let index = self.entries.partition_point(|r| route_priority(r) >= key);
        self.entries.insert(index, route);

        debug_assert!(
            self.entries
                .is_sorted_by_key(|r| cmp::Reverse(route_priority(r)))
        );
    }

    /// Removes the route at `index` and returns it.
    fn remove(&mut self, index: usize) -> RouteEntry {
        self.entries.remove(index)
    }

    /// Removes the `route` if owned by user.
    fn remove_user_route(&mut self, route: &RouteEntry) -> Result<()> {
        let index = self.position(route).ok_or(NetError::RouteNotFound)?;

        if self.entries[index].owner == RouteOwner::Kernel {
            err!(NetError::NotPermitted)
        }

        self.remove(index);

        Ok(())
    }

    /// Checks if `addr` has `Local` route, on `interface_id` if given.
    fn has_local_route(&self, addr: Ipv4Addr, interface_id: Option<InterfaceId>) -> bool {
        self.entries.iter().any(|r| {
            r.kind == RouteKind::Local
                && r.dest_ip == addr
                && interface_id.is_none_or(|i| r.interface_id == i)
        })
    }

    /// Checks if `gateway` is reachable without another gateway, on the same interface.
    fn is_on_link(&self, gateway: Ipv4Addr, interface_id: InterfaceId) -> bool {
        self.entries.iter().any(|r| {
            r.interface_id == interface_id
                && r.gateway.is_none()
                && r.kind == RouteKind::Unicast
                && route_matches(gateway, r.dest_ip, r.prefix_len)
        })
    }

    /// Removes this interface's routes that fail `valid`.
    fn retain_on_interface(
        &mut self,
        interface_id: InterfaceId,
        keep: fn(&RouteTable, &RouteEntry) -> bool,
    ) {
        let mut i = 0;
        while i < self.entries.len() {
            let route = self.entries[i];
            if route.interface_id == interface_id && !keep(self, &route) {
                self.entries.remove(i);
            } else {
                i += 1;
            }
        }
    }

    /// Removes every route on this interface that `add_route` would reject.
    fn prune_interface(&mut self, interface_id: InterfaceId) {
        // `pref_src` first since it can remove direct routes that gateway might depend on.
        self.retain_on_interface(interface_id, |table, r| {
            r.pref_src
                .is_none_or(|a| table.has_local_route(a, Some(r.interface_id)))
        });

        self.retain_on_interface(interface_id, |table, r| {
            r.gateway
                .is_none_or(|g| table.is_on_link(g, r.interface_id))
        });
    }

    /// Finds the best matching route for a given destination IP address on the interface, based on
    /// `route_priority()`.
    /// Returns `None` if no matching route is found.
    fn best(&self, dest_ip: Ipv4Addr, interface_id: Option<InterfaceId>) -> Option<RouteEntry> {
        // routes are already sorted on insert.
        self.entries
            .iter()
            .filter(|r| interface_id.is_none_or(|i| r.interface_id == i))
            .find(|r| route_matches(dest_ip, r.dest_ip, r.prefix_len))
            .copied()
    }
}

/// Checks if a given destination IP address matches a route's subnet, based on the route's
/// destination and prefix length.
fn route_matches(dest_ip: Ipv4Addr, route_ip: Ipv4Addr, route_prefix_len: u8) -> bool {
    let mask = Ipv4Addr::prefix_len_to_mask(route_prefix_len).expect("prefix length to be valid");
    (dest_ip & mask) == (route_ip & mask)
}

/// Priority used to keep ROUTES in lookup order. Higher is preferred.
fn route_priority(route: &RouteEntry) -> (u8, cmp::Reverse<u32>) {
    // Sort by longest prefix length first, then by lowest metric.
    (route.prefix_len, cmp::Reverse(route.metric))
}

/// Returns if `addr` has a `Local` route, on `interface_id` if given.
pub fn is_local_addr(addr: Ipv4Addr, interface_id: Option<InterfaceId>) -> bool {
    ROUTES.lock().has_local_route(addr, interface_id)
}

/// Returns a copy of the route entry at a given index in lookup order (best is first).
pub fn route_at(index: usize) -> Option<RouteEntry> {
    ROUTES.lock().get(index)
}

/// Adds a new route after validating it.
/// If `replace` is true, when a route with the same dest, prefix, gateway, and interface already
/// exists, it will be updated with the new information.
/// Otherwise, a new route entry will be added to the routing table.
pub fn add_route(route: RouteEntry, replace: bool) -> Result<()> {
    // must be a valid mask
    let Some(mask) = Ipv4Addr::prefix_len_to_mask(route.prefix_len) else {
        err!(NetError::InvalidAddress)
    };

    // mask must match the dest_ip
    if route.dest_ip & mask != route.dest_ip {
        err!(NetError::InvalidAddress)
    }

    // must not be a kernel-owned or local route
    if route.owner == RouteOwner::Kernel || route.kind != RouteKind::Unicast {
        err!(NetError::NotPermitted)
    }

    // interface must exists
    if interface::find_interface_by_id(route.interface_id).is_none() {
        err!(NetError::InterfaceNotFound)
    };

    let mut routes = ROUTES.lock();

    if let Some(pref_src) = route.pref_src {
        // a pref_src addr must be owned by the interface.
        if !routes.has_local_route(pref_src, Some(route.interface_id)) {
            err!(NetError::AddressUnavailable)
        }
    }

    if let Some(gateway) = route.gateway {
        // a gateway must be reachable without another gateway, on the same interface.
        if !routes.is_on_link(gateway, route.interface_id) {
            err!(NetError::GatewayUnreachable)
        }
    }

    if routes.position(&route).is_some() {
        if !replace {
            err!(NetError::RouteExists);
        }

        routes.remove_user_route(&route)?;
    }

    routes.insert(route);

    Ok(())
}

/// Removes the route matching dest, prefix, gateway, and interface.
/// Kernel owned routes cannot be removed this way.
/// After successful removal, prunes the other routes on this interface.
pub fn remove_route(route: &RouteEntry) -> Result<()> {
    let mut routes = ROUTES.lock();

    routes.remove_user_route(route)?;
    routes.prune_interface(route.interface_id);

    Ok(())
}

/// Removes all routes of a given interface.
/// Used when interface loses its IP assignment, then every route is obsolete.
pub fn remove_interface_routes(interface_id: InterfaceId) {
    ROUTES
        .lock()
        .retain_on_interface(interface_id, |_, _| false)
}

/// Removes unreachable routes owned by this interface and adds a new route for the interface's IP
/// address and prefix.
pub fn replace_interface_route(interface_id: InterfaceId, ipv4: Ipv4Cidr) {
    let mut routes = ROUTES.lock();

    // Remove any kernel owned route for this interface.
    routes.retain_on_interface(interface_id, |_, r| r.owner != RouteOwner::Kernel);

    // Add a new route for local route for loopback.
    routes.insert(RouteEntry {
        dest_ip: ipv4.addr,
        prefix_len: 32,
        gateway: None,
        interface_id, // use the actual interface_id not loopback
        metric: 0,
        owner: RouteOwner::Kernel,
        kind: RouteKind::Local,
        pref_src: Some(ipv4.addr),
    });

    // Add a new route for the interface's IP address and prefix.
    routes.insert(RouteEntry {
        dest_ip: ipv4.addr
            & Ipv4Addr::prefix_len_to_mask(ipv4.prefix_len).expect("valid prefix length"),
        prefix_len: ipv4.prefix_len,
        gateway: None,
        interface_id,
        metric: 10,
        owner: RouteOwner::Kernel,
        kind: RouteKind::Unicast,
        pref_src: None,
    });

    routes.prune_interface(interface_id);
}

/// Finds the best matching route for a given destination IP address on the interface, based on the
/// longest prefix match and lowest metric.
///
/// Returns `None` if no matching route is found.
pub fn best_route_for(dest_ip: Ipv4Addr, interface_id: Option<InterfaceId>) -> Option<RouteEntry> {
    ROUTES.lock().best(dest_ip, interface_id)
}

/// Returns the interface a packet matching `route` is actually sent on.
pub fn egress_interface(route: &RouteEntry) -> InterfaceId {
    match route.kind {
        RouteKind::Local => loopback::interface_id(),
        RouteKind::Unicast => route.interface_id,
    }
}

/// Prints the current routing table for debugging purposes.
pub fn dump() {
    // header and rows share one format string so the columns always line up.
    // widths fit the longest value of each column, e.g. "255.255.255.255/32" and "kernel".
    macro_rules! row {
        ($dest:expr, $gateway:expr, $dev:expr, $metric:expr, $proto:expr, $scope:expr, $src:expr) => {
            println!(
                "{:<18} {:<15} {:<6} {:<6} {:<6} {:<6} {}",
                $dest, $gateway, $dev, $metric, $proto, $scope, $src
            )
        };
    }

    let routes = ROUTES.lock().entries.clone();

    println!("");
    row!(
        "DESTINATION",
        "GATEWAY",
        "DEVICE",
        "METRIC",
        "OWNER",
        "SCOPE",
        "SOURCE"
    );

    for route in routes {
        let interface = interface::find_interface_by_id(route.interface_id);
        let device = interface.as_ref().map_or("-", |i| i.config.name);
        let src = interface
            .as_ref()
            .and_then(|i| net::source_addr(&route, i))
            .map_or("-".to_string(), |s| s.to_string());

        let dest = Ipv4Cidr {
            addr: route.dest_ip,
            prefix_len: route.prefix_len,
        };
        let gateway = route
            .gateway
            .map_or("direct".to_string(), |gw| gw.to_string());
        let scope = if route.kind == RouteKind::Local {
            "host"
        } else if route.gateway.is_some() {
            "global"
        } else {
            "link"
        };

        row!(dest, gateway, device, route.metric, route.owner, scope, src);
    }
}
