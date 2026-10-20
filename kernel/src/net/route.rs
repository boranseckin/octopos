use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::net::interface::{self, InterfaceId};
use crate::net::{self, Ipv4Addr, Ipv4Config, NetError, loopback};
use crate::spinlock::SpinLock;

/// Owner of a route entry, used for tracking which component added the route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteOwner {
    /// Route added by an interface's address and prefix.
    Interface,
    /// Route to one of the host's own addresses via loopback.
    Local(InterfaceId),
    /// Route added by the DHCP client when it receives an IP address lease.
    Dhcp,
    /// Route added by a static configuration.
    _Static,
}

impl core::fmt::Display for RouteOwner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RouteOwner::Interface => write!(f, "if"),
            RouteOwner::Local(interface_id) => write!(f, "local:{interface_id}"),
            RouteOwner::Dhcp => write!(f, "dhcp"),
            RouteOwner::_Static => write!(f, "static"),
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
    /// Interface to send packets matching this route on.
    pub interface_id: InterfaceId,
    /// Routing metric for this route, used to determine the best route when multiple routes match a
    /// destination. Lower values indicate more preferred routes.
    pub metric: u32,
    /// Owner of this route, used for tracking which component added this route and allowing it to
    /// remove the route later if needed.
    pub owner: RouteOwner,
    /// Preferred source address for packets using this route. If `None`, the outgoing interface's
    /// address is used.
    pub pref_src: Option<Ipv4Addr>,
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

/// Adds or updates a route entry in the routing table.
///
/// If a route with the same destination and prefix already exists, it will be updated with the new
/// information. Otherwise, a new route entry will be added to the routing table.
pub fn upsert_route(route: RouteEntry) {
    let mut routes = ROUTES.lock();

    if let Some(existing_route) = routes.iter_mut().find(|r| route.eq(*r)) {
        *existing_route = route;
    } else {
        routes.push(route);
    }
}

/// Removes all routes owned by the specified interface and adds a new route for the interface's IP
/// address and prefix.
pub fn replace_interface_route(interface_id: InterfaceId, ipv4: Ipv4Config) {
    let mut routes = ROUTES.lock();

    // Remove any existing Interface owned route or local route to this interface.
    routes.retain(|r| {
        !matches!(r.owner, RouteOwner::Interface if r.interface_id == interface_id)
            && r.owner != RouteOwner::Local(interface_id)
    });

    // Add a new route for the interface's local loopback.
    routes.push(RouteEntry {
        dest_ip: ipv4.addr,
        prefix_len: 32,
        gateway: None,
        interface_id: loopback::interface_id(),
        metric: 0,
        owner: RouteOwner::Local(interface_id),
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
        owner: RouteOwner::Interface,
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
        err!(NetError::RouteNotFound)
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
        "PROTO",
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
        let owner = match route.owner {
            RouteOwner::Interface | RouteOwner::Local(_) => "kernel",
            RouteOwner::Dhcp => "dhcp",
            RouteOwner::_Static => "static",
        };
        let scope = if matches!(route.owner, RouteOwner::Local(_)) {
            "host"
        } else if route.gateway.is_some() {
            "global"
        } else {
            "link"
        };

        row!(dest, gateway, device, route.metric, owner, scope, src);
    }
}
