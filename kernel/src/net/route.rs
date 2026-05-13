use alloc::string::ToString;
use alloc::vec::Vec;

use crate::net::interface::{self, InterfaceId};
use crate::net::{Ipv4Addr, Ipv4Config, NetError};
use crate::spinlock::SpinLock;

/// Owner of a route entry, used for tracking which component added the route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteOwner {
    /// Route added by an interface's address and prefix.
    Interface,
    /// Route added by the DHCP client when it receives an IP address lease.
    Dhcp,
    /// Route added by a static configuration.
    _Static,
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

    // Remove any existing Interface owned route.
    routes.retain(|r| !(r.interface_id == interface_id && r.owner == RouteOwner::Interface));

    // Add a new route for the interface's IP address and prefix.
    routes.push(RouteEntry {
        dest_ip: ipv4.addr
            & Ipv4Addr::prefix_len_to_mask(ipv4.prefix_len).expect("valid prefix length"),
        prefix_len: ipv4.prefix_len,
        gateway: None,
        interface_id,
        metric: 10,
        owner: RouteOwner::Interface,
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
    let routes = ROUTES.lock();

    println!("");
    for route in routes.iter() {
        println!(
            "{}/{} via {} dev {} metric {} owner {:?}",
            route.dest_ip,
            route.prefix_len,
            route
                .gateway
                .map(|gw| gw.to_string())
                .unwrap_or_else(|| "direct".to_string()),
            interface::find_interface_by_id(route.interface_id)
                .unwrap()
                .config
                .name,
            route.metric,
            route.owner
        );
    }
}
