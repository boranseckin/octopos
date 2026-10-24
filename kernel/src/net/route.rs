use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::net::interface::{self, InterfaceId};
use crate::net::loopback;
use crate::net::{self, Ipv4Addr, Ipv4Cidr, NetError};
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

impl TryFrom<u8> for RouteOwner {
    type Error = NetError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Kernel),
            2 => Ok(Self::Dhcp),
            3 => Ok(Self::Static),
            _ => err!(NetError::NotPermitted),
        }
    }
}

impl core::fmt::Display for RouteOwner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RouteOwner::Kernel => write!(f, "kernel"),
            RouteOwner::Dhcp => write!(f, "dhcp"),
            RouteOwner::Static => write!(f, "static"),
        }
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

impl TryFrom<u8> for RouteKind {
    type Error = NetError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Unicast),
            2 => Ok(Self::Local),
            _ => err!(NetError::NotPermitted),
        }
    }
}

impl core::fmt::Display for RouteKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RouteKind::Unicast => write!(f, "unicast"),
            RouteKind::Local => write!(f, "local"),
        }
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

    fn try_from(value: RawRoute) -> Result<Self, Self::Error> {
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
            owner: try_log!(RouteOwner::try_from(value.owner)),
            kind: try_log!(RouteKind::try_from(value.kind)),
            pref_src: if value.pref_src == [0; 4] {
                None
            } else {
                Some(Ipv4Addr::from(value.pref_src))
            },
        })
    }
}

static ROUTES: SpinLock<Vec<RouteEntry>> = SpinLock::new(Vec::new(), "net_routes");

impl PartialEq for RouteEntry {
    fn eq(&self, other: &Self) -> bool {
        self.dest_ip == other.dest_ip
            && self.prefix_len == other.prefix_len
            && self.gateway == other.gateway
            && self.interface_id == other.interface_id
    }
}

/// Checks if a given destination IP address matches a route entry, based on the route's destination
/// and prefix length.
pub fn route_matches(dest_ip: Ipv4Addr, route: &RouteEntry) -> bool {
    let mask = Ipv4Addr::prefix_len_to_mask(route.prefix_len).expect("prefix length to be valid");
    (dest_ip & mask) == (route.dest_ip & mask)
}

/// Generates a sort key for a route entry, used to sort routes by their prefix length and metric.
pub fn route_sort_key(route: &RouteEntry) -> (u8, core::cmp::Reverse<u32>) {
    // Sort by longest prefix length first, then by lowest metric.
    (route.prefix_len, core::cmp::Reverse(route.metric))
}

/// Returns a copy of the route entry at a given index.
pub fn route_at(index: usize) -> Result<RouteEntry, NetError> {
    if let Some(route) = ROUTES.lock().get(index) {
        Ok(*route)
    } else {
        err!(NetError::RouteNotFound)
    }
}

/// Adds a new route after validating it.
/// If `replace` is true, when a route with the same dest, prefix, gateway, and interface already
/// exists, it will be updated with the new information.
/// Otherwise, a new route entry will be added to the routing table.
pub fn add_route(route: RouteEntry, replace: bool) -> Result<(), NetError> {
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

    if let Some(gateway) = route.gateway {
        // a gateway must be reachable without another gateway, on the same interface
        if !routes.iter().any(|r| {
            r.interface_id == route.interface_id
                && r.gateway.is_none()
                && r.kind == RouteKind::Unicast
                && route_matches(gateway, r)
        }) {
            err!(NetError::GatewayUnreachable)
        }
    }

    if let Some(existing) = routes.iter_mut().find(|r| route.eq(*r)) {
        if !replace {
            err!(NetError::RouteExists);
        }

        if existing.owner == RouteOwner::Kernel {
            err!(NetError::NotPermitted)
        }

        // overwrite existing route
        *existing = route;
    } else {
        routes.push(route);
    }

    Ok(())
}

/// Removes the route matching dest, prefix, gateway, and interface.
/// Kernel owned routes cannot be removed this way.
pub fn remove_route(route: &RouteEntry) -> Result<(), NetError> {
    let mut routes = ROUTES.lock();

    let Some((i, entry)) = routes.iter().enumerate().find(|(_, r)| route.eq(r)) else {
        err!(NetError::RouteNotFound)
    };

    if entry.owner == RouteOwner::Kernel {
        err!(NetError::NotPermitted)
    }

    routes.remove(i);

    Ok(())
}

/// Removes all routes of a given interface.
/// Used when interface loses its IP assignment, then every route is obsolete.
pub fn remove_interface_routes(interface_id: InterfaceId) {
    ROUTES.lock().retain(|r| r.interface_id != interface_id);
}

/// Removes all routes owned by the specified interface and adds a new route for the interface's IP
/// address and prefix.
pub fn replace_interface_route(interface_id: InterfaceId, ipv4: Ipv4Cidr) {
    let mut routes = ROUTES.lock();

    // Remove any existing kernel owned route for this interface.
    routes.retain(|r| !matches!(r.owner, RouteOwner::Kernel if r.interface_id == interface_id));

    // Add a new route for local route for loopback.
    routes.push(RouteEntry {
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
    routes.push(RouteEntry {
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
}

/// Finds the best matching route for a given destination IP address, based on the longest prefix
/// match and lowest metric.
///
/// Returns `None` if no matching route is found.
pub fn best_route_for(dest_ip: Ipv4Addr) -> Result<RouteEntry, NetError> {
    let routes = ROUTES.lock();

    // Find all routes that match the destination,
    // then sort them by longest prefix and lowest metric.
    if let Some(route) = routes
        .iter()
        .filter(|r| route_matches(dest_ip, r))
        .max_by_key(|r| route_sort_key(r))
    {
        Ok(*route)
    } else {
        err!(NetError::NetworkUnreachable)
    }
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

    let mut routes = ROUTES.lock().clone();
    routes.sort_by_key(|r| core::cmp::Reverse(route_sort_key(r)));

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

    for route in routes.iter() {
        let interface = interface::find_interface_by_id(route.interface_id);
        let device = interface.as_ref().map_or("-", |i| i.config.name);
        let src = interface
            .as_ref()
            .and_then(|i| net::source_addr(route, i))
            .map_or("-".to_string(), |s| s.to_string());

        let dest = format!("{}/{}", route.dest_ip, route.prefix_len);
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
