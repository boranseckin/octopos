#![no_std]
#![no_main]

use user::*;

// All tests only touch lo and routes through lo, so a failure cannot cut the host link.
// Tests that change lo's address restore 127.0.0.1/8 before returning.

const MAX_ROUTES: usize = 32;
const MAX_IF: usize = 32;

const LO_ADDR: Ipv4Addr = Ipv4Addr([127, 0, 0, 1]);
const LO_PREFIX: u8 = 8;

/// Gateway on lo's 127.0.0.0/8, so routes through it pass the on-link check.
const LO_GATEWAY: Ipv4Addr = Ipv4Addr([127, 0, 0, 2]);

/// Destination used by the tests. Outside every kernel route, so it never shadows real traffic.
const TEST_DEST: Ipv4Addr = Ipv4Addr([10, 99, 0, 0]);
const TEST_PREFIX: u8 = 16;

fn routes() -> [Option<RouteEntry>; MAX_ROUTES] {
    let mut routes = [None; MAX_ROUTES];

    for (i, route) in routes.iter_mut().enumerate() {
        match route_get(i) {
            Ok(r) => *route = Some(r),
            Err(SysError::NoEntry) => break,
            Err(e) => panic!("route_get({i}): {e:?}"),
        }
    }

    routes
}

fn interfaces_raw() -> [Option<RawInterface>; MAX_IF] {
    let mut ifs = [None; MAX_IF];

    for (i, interface) in ifs.iter_mut().enumerate() {
        match interface_get(i) {
            Ok(raw) => *interface = Some(raw),
            Err(SysError::NoEntry) => break,
            Err(e) => panic!("interface_get({i}): {e:?}"),
        }
    }

    ifs
}

fn lo() -> Interface {
    loopback().expect("loopback interface").unwrap()
}

fn lo_id() -> InterfaceId {
    lo().id
}

/// A non-loopback interface, if the kernel has one (e1000 under QEMU).
fn other_id() -> Option<InterfaceId> {
    interface_iter()
        .flatten()
        .find(|&i| !i.is_loopback())
        .map(|i| i.id)
}

/// Finds a route by its key: destination, prefix, gateway and interface.
fn find_route(
    dest_ip: Ipv4Addr,
    prefix_len: u8,
    gateway: Option<Ipv4Addr>,
    interface_id: InterfaceId,
) -> Option<RouteEntry> {
    routes().into_iter().flatten().find(|r| {
        r.dest_ip == dest_ip
            && r.prefix_len == prefix_len
            && r.gateway == gateway
            && r.interface_id == interface_id
    })
}

/// Compares every field, unlike `RouteEntry`'s `PartialEq` which only compares the key.
fn same_route(a: &RouteEntry, b: &RouteEntry) -> bool {
    a.dest_ip == b.dest_ip
        && a.prefix_len == b.prefix_len
        && a.gateway == b.gateway
        && a.interface_id == b.interface_id
        && a.metric == b.metric
        && a.owner == b.owner
        && a.kind == b.kind
        && a.pref_src == b.pref_src
}

/// True if both tables hold the same routes, in any order.
fn same_routes(a: &[Option<RouteEntry>], b: &[Option<RouteEntry>]) -> bool {
    let count = |t: &[Option<RouteEntry>]| t.iter().flatten().count();

    count(a) == count(b)
        && a.iter()
            .flatten()
            .all(|ra| b.iter().flatten().any(|rb| same_route(ra, rb)))
}

/// Static route to `TEST_DEST` via `LO_GATEWAY` on lo.
fn test_route() -> RouteEntry {
    RouteEntry {
        dest_ip: TEST_DEST,
        prefix_len: TEST_PREFIX,
        gateway: Some(LO_GATEWAY),
        interface_id: lo_id(),
        metric: 100,
        owner: RouteOwner::Static,
        kind: RouteKind::Unicast,
        pref_src: None,
    }
}

fn restore_lo() {
    interface_set_ipv4(lo_id(), LO_ADDR, LO_PREFIX).expect("restore lo address");
}

/// lo is listed with its fixed address and flags, and listing stops with NoEntry.
fn test_interface_list() {
    let ifs = interfaces_raw();
    let count = ifs.iter().flatten().count();
    assert!(count >= 1, "at least lo must be listed");

    let lo = lo();
    let cidr = lo.ipv4.expect("lo must have an address");
    assert_eq!(lo.name(), "lo", "loopback must be named lo");
    assert!(
        lo.is_up() && lo.is_configured() && lo.is_loopback(),
        "lo must be up, loopback and configured"
    );
    assert_eq!(cidr.addr, LO_ADDR, "lo address mismatch");
    assert_eq!(cidr.prefix_len, LO_PREFIX, "lo prefix mismatch");

    // ids are unique
    for (i, a) in ifs.iter().flatten().enumerate() {
        for b in ifs.iter().flatten().skip(i + 1) {
            assert_ne!(a.id, b.id, "interface ids must be unique");
        }
    }

    assert_eq!(
        interface_get(count).map(|raw| raw.id),
        Err(SysError::NoEntry),
        "past the end must return NoEntry"
    );
}

/// The kernel creates a local /32 and a subnet route for lo's address.
fn test_route_list_kernel_routes() {
    let lo = lo_id();

    let local = find_route(LO_ADDR, 32, None, lo).expect("lo local route must exist");
    assert_eq!(local.owner, RouteOwner::Kernel, "local route owner");
    assert_eq!(local.kind, RouteKind::Local, "local route kind");
    assert_eq!(local.pref_src, Some(LO_ADDR), "local route source");

    let subnet = find_route(Ipv4Addr([127, 0, 0, 0]), LO_PREFIX, None, lo)
        .expect("lo subnet route must exist");
    assert_eq!(subnet.owner, RouteOwner::Kernel, "subnet route owner");
    assert_eq!(subnet.kind, RouteKind::Unicast, "subnet route kind");

    let count = routes().iter().flatten().count();
    assert_eq!(
        route_get(count).map(|r| r.metric),
        Err(SysError::NoEntry),
        "past the end must return NoEntry"
    );
}

/// A static route can be added, replaced and removed exactly once.
fn test_route_add_replace_remove() {
    let route = test_route();

    route_add(&route, 0).expect("route_add");
    let added = find_route(TEST_DEST, TEST_PREFIX, Some(LO_GATEWAY), lo_id())
        .expect("added route must be listed");
    assert!(
        same_route(&added, &route),
        "listed route must match what was added"
    );

    assert_eq!(
        route_add(&route, 0),
        Err(SysError::AlreadyExists),
        "adding the same route again must fail"
    );

    let replacement = RouteEntry { metric: 7, ..route };
    route_add(&replacement, RouteFlag::ROUTE_REPLACE).expect("route_add replace");
    let replaced = find_route(TEST_DEST, TEST_PREFIX, Some(LO_GATEWAY), lo_id())
        .expect("replaced route must be listed");
    assert_eq!(replaced.metric, 7, "replace must update the metric");

    let count = routes()
        .iter()
        .flatten()
        .filter(|r| r.dest_ip == TEST_DEST && r.prefix_len == TEST_PREFIX)
        .count();
    assert_eq!(count, 1, "replace must not add a second route");

    route_remove(&route).expect("route_remove");
    assert!(
        find_route(TEST_DEST, TEST_PREFIX, Some(LO_GATEWAY), lo_id()).is_none(),
        "removed route must be gone"
    );

    assert_eq!(
        route_remove(&route),
        Err(SysError::NoEntry),
        "removing a missing route must fail"
    );
}

/// A static route is used for sending, and removing it makes the destination unreachable.
fn test_route_is_used() {
    let fd = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket open");
    let dest = SockAddr::inet(Ipv4Addr([10, 99, 0, 1]), 9);

    // with a default route (e.g. from DHCP) every destination is reachable, so only check
    // unreachability when there is none
    let has_default = routes().iter().flatten().any(|r| r.prefix_len == 0);
    if !has_default {
        assert_eq!(
            send(fd, b"x", dest),
            Err(SysError::NetworkUnreachable),
            "send without a route must be unreachable"
        );
    }

    let route = test_route();
    route_add(&route, 0).expect("route_add");
    assert_eq!(send(fd, b"x", dest), Ok(1), "send through the static route");
    route_remove(&route).expect("route_remove");

    if !has_default {
        assert_eq!(
            send(fd, b"x", dest),
            Err(SysError::NetworkUnreachable),
            "send after removing the route must be unreachable"
        );
    }

    close(fd).expect("close");
}

/// Invalid routes are rejected, and none of them end up in the table.
fn test_route_add_rejects() {
    let before = routes();
    let route = test_route();

    assert_eq!(
        route_add(
            &RouteEntry {
                prefix_len: 33,
                ..route
            },
            0
        ),
        Err(SysError::InvalidArgument),
        "prefix over 32 must be rejected"
    );

    assert_eq!(
        route_add(
            &RouteEntry {
                dest_ip: Ipv4Addr([10, 99, 0, 5]),
                ..route
            },
            0
        ),
        Err(SysError::InvalidArgument),
        "host bits outside the prefix must be rejected"
    );

    assert_eq!(
        route_add(
            &RouteEntry {
                interface_id: InterfaceId(999),
                ..route
            },
            0
        ),
        Err(SysError::NoEntry),
        "unknown interface must be rejected"
    );

    assert_eq!(
        route_add(
            &RouteEntry {
                gateway: Some(Ipv4Addr([10, 1, 1, 1])),
                ..route
            },
            0
        ),
        Err(SysError::InvalidArgument),
        "gateway not on any direct route must be rejected"
    );

    if let Some(other) = other_id() {
        assert_eq!(
            route_add(
                &RouteEntry {
                    interface_id: other,
                    ..route
                },
                0
            ),
            Err(SysError::InvalidArgument),
            "gateway on-link on a different interface must be rejected"
        );
    }

    assert_eq!(
        route_add(
            &RouteEntry {
                owner: RouteOwner::Kernel,
                ..route
            },
            0
        ),
        Err(SysError::NotPermitted),
        "kernel-owned routes cannot be added"
    );

    assert_eq!(
        route_add(
            &RouteEntry {
                kind: RouteKind::Local,
                ..route
            },
            0
        ),
        Err(SysError::NotPermitted),
        "local routes cannot be added"
    );

    // a gateway reachable only through another gateway is not on-link
    route_add(&route, 0).expect("route_add");
    let via_gateway = RouteEntry {
        dest_ip: Ipv4Addr([10, 98, 0, 0]),
        gateway: Some(Ipv4Addr([10, 99, 0, 1])),
        ..route
    };
    assert_eq!(
        route_add(&via_gateway, 0),
        Err(SysError::InvalidArgument),
        "gateway behind another gateway must be rejected"
    );
    route_remove(&route).expect("route_remove");

    assert!(
        same_routes(&before, &routes()),
        "rejected routes must not change the table"
    );
}

/// Kernel routes cannot be removed or replaced, whatever owner the request claims.
fn test_kernel_routes_protected() {
    let before = routes();

    let lo_subnet = RouteEntry {
        dest_ip: Ipv4Addr([127, 0, 0, 0]),
        prefix_len: LO_PREFIX,
        gateway: None,
        interface_id: lo_id(),
        metric: 10,
        owner: RouteOwner::Static, // a lie, the stored route is Kernel
        kind: RouteKind::Unicast,
        pref_src: None,
    };

    assert_eq!(
        route_remove(&lo_subnet),
        Err(SysError::NotPermitted),
        "removing a kernel route must fail even when the request claims Static"
    );

    assert_eq!(
        route_add(&lo_subnet, 0),
        Err(SysError::AlreadyExists),
        "adding over a kernel route without replace must fail"
    );

    assert_eq!(
        route_add(&lo_subnet, RouteFlag::ROUTE_REPLACE),
        Err(SysError::NotPermitted),
        "replacing a kernel route must fail"
    );

    assert!(
        same_routes(&before, &routes()),
        "kernel routes must be unchanged"
    );
}

/// Invalid addresses are rejected and leave lo untouched.
fn test_set_ipv4_rejects() {
    let before = routes();

    assert_eq!(
        interface_set_ipv4(lo_id(), LO_ADDR, 0),
        Err(SysError::InvalidArgument),
        "prefix 0 must be rejected"
    );

    assert_eq!(
        interface_set_ipv4(lo_id(), LO_ADDR, 33),
        Err(SysError::InvalidArgument),
        "prefix over 32 must be rejected"
    );

    assert_eq!(
        interface_set_ipv4(InterfaceId(999), LO_ADDR, LO_PREFIX),
        Err(SysError::NoEntry),
        "unknown interface must be rejected"
    );

    let lo = lo();
    let cidr = lo.ipv4.expect("lo must have an address");
    assert_eq!(cidr.addr, LO_ADDR, "lo address must be unchanged");
    assert_eq!(cidr.prefix_len, LO_PREFIX, "lo prefix must be unchanged");
    assert!(same_routes(&before, &routes()), "routes must be unchanged");
}

/// Setting the address lo already has rebuilds the same kernel routes and keeps static ones.
fn test_set_ipv4_idempotent() {
    let route = test_route();
    route_add(&route, 0).expect("route_add");

    let before = routes();
    restore_lo();
    assert!(
        same_routes(&before, &routes()),
        "setting the same address must leave the table unchanged"
    );

    route_remove(&route).expect("route_remove");
}

/// Clearing an address removes every route on the interface; setting it again restores the
/// kernel routes but not the static ones.
fn test_clear_and_restore() {
    let route = test_route();
    route_add(&route, 0).expect("route_add");

    interface_set_ipv4(lo_id(), Ipv4Addr::UNSPECIFIED, 0).expect("clear lo");

    let cleared = lo();
    let unconfigured = !cleared.is_configured();
    let no_routes = routes().iter().flatten().all(|r| r.interface_id != lo_id());

    // restore before asserting, so a failure here does not leave lo broken
    restore_lo();

    assert!(unconfigured, "cleared lo must not be configured");
    assert!(no_routes, "clearing must remove every route on lo");

    assert!(
        find_route(LO_ADDR, 32, None, lo_id()).is_some(),
        "restoring must recreate the local route"
    );
    assert!(
        find_route(Ipv4Addr([127, 0, 0, 0]), LO_PREFIX, None, lo_id()).is_some(),
        "restoring must recreate the subnet route"
    );
    assert!(
        find_route(TEST_DEST, TEST_PREFIX, Some(LO_GATEWAY), lo_id()).is_none(),
        "static route removed by the clear must not come back"
    );
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_interface_list();
    test_route_list_kernel_routes();
    test_route_add_replace_remove();
    test_route_is_used();
    test_route_add_rejects();
    test_kernel_routes_protected();
    test_set_ipv4_rejects();
    test_set_ipv4_idempotent();
    test_clear_and_restore();
}
