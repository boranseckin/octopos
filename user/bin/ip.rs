#![no_std]
#![no_main]
#![feature(anonymous_lifetime_in_impl_trait)]

use core::{fmt::Display, str::FromStr};

use user::*;

const USAGE: &str = "usage: ip addr
       ip addr set <dev> <addr>/<prefix>
       ip addr clear <dev>
       ip route
       ip route add <dest>/<prefix> [via <gw>] dev <dev> [metric <n>] [src <addr>] [replace]
       ip route remove <dest>/<prefix> [via <gw>] dev <dev>";

const MAX_INTERFACES: usize = 16;
const DEFAULT_METRIC: u32 = 100;

#[unsafe(no_mangle)]
fn main(args: Args) {
    let mut rest = args.iter_as_str().skip(3);

    match (args.get_str(1), args.get_str(2)) {
        (Some("addr"), None) => addr_list_cmd(),
        (Some("addr"), Some("set")) => addr_set_cmd(&mut rest),
        (Some("addr"), Some("clear")) => addr_clear_cmd(&mut rest),
        (Some("route"), None) => route_list_cmd(),
        (Some("route"), Some("add")) => route_add_cmd(&mut rest),
        (Some("route"), Some("remove")) => route_remove_cmd(&mut rest),
        _ => exit_with_msg(USAGE),
    }
}

fn next_arg<'a>(rest: &mut impl Iterator<Item = &'a str>) -> &'a str {
    rest.next().unwrap_or_else(|| exit_with_msg(USAGE))
}

fn interfaces() -> [Option<RawInterface>; MAX_INTERFACES] {
    let mut ifs = [None; MAX_INTERFACES];

    for (i, interface) in ifs.iter_mut().enumerate() {
        match interface_get(i) {
            Ok(raw) => interface.replace(raw),
            Err(SysError::NoEntry) => break,
            Err(e) => exit_with_msg(e),
        };
    }

    ifs
}

fn find_interface(id: InterfaceId, ifs: &[Option<RawInterface>]) -> Option<&RawInterface> {
    ifs.iter().flatten().find(|i| i.id == id.0)
}

fn resolve_dev(s: &str) -> InterfaceId {
    if let Ok(n) = s.parse() {
        return InterfaceId(n);
    }

    match interfaces().iter().flatten().find(|i| name_str(i) == s) {
        Some(i) => InterfaceId(i.id),
        None => exit_with_msg(format_args!("unknown device: {s}")),
    }
}

fn cidr(raw: &RawInterface) -> Option<Ipv4Cidr> {
    (raw.flags & InterfaceFlag::CONFIGURED != 0).then(|| Ipv4Cidr {
        addr: Ipv4Addr::from(raw.addr),
        prefix_len: raw.prefix_len,
    })
}

fn name_str(raw: &RawInterface) -> &str {
    let len = raw
        .name
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(raw.name.len());
    str::from_utf8(&raw.name[..len]).unwrap_or("?")
}

fn addr_list_cmd() {
    println!("{} {} {} {} {}", "ID", "DEVICE", "MAC", "ADDRESS", "STATE");

    for raw in interfaces().iter().flatten() {
        let cidr = cidr(raw);
        let addr: &dyn Display = match &cidr {
            Some(c) => c,
            None => &"unconfigured",
        };

        let up = if raw.flags & InterfaceFlag::UP != 0 {
            "up"
        } else {
            "down"
        };
        let loopback = if raw.flags & InterfaceFlag::LOOPBACK != 0 {
            "loopback"
        } else {
            ""
        };

        println!(
            "{} {} {} {} {} {}",
            raw.id,
            name_str(raw),
            MacAddr(raw.mac),
            addr,
            up,
            loopback
        );
    }
}

fn addr_set_cmd(rest: &mut impl Iterator<Item = &str>) {
    let id = resolve_dev(next_arg(rest));
    let Ipv4Cidr { addr, prefix_len } =
        Ipv4Cidr::from_str(next_arg(rest)).unwrap_or_else(|_| exit_with_msg("invalid address"));

    if addr == Ipv4Addr::UNSPECIFIED {
        exit_with_msg("use `ip addr clear` to remove an address");
    }

    interface_set_ipv4(id, addr, prefix_len).unwrap_or_else(|e| exit_with_msg(e))
}

fn addr_clear_cmd(rest: &mut impl Iterator<Item = &str>) {
    let id = resolve_dev(next_arg(rest));
    interface_set_ipv4(id, Ipv4Addr::UNSPECIFIED, 0).unwrap_or_else(|e| exit_with_msg(e))
}

fn route_list_cmd() {
    // read interfaces once.
    // this might get stale while we are enumerating routes.
    let ifs = interfaces();

    println!(
        "{} {} {} {} {} {} {}",
        "DEST", "GATEWAY", "DEVICE", "METRIC", "OWNER", "SCOPE", "SRC"
    );

    for i in 0.. {
        let route = match route_get(i) {
            Ok(r) => r,
            Err(SysError::NoEntry) => break,
            Err(e) => exit_with_msg(e),
        };

        let addr = Ipv4Cidr {
            addr: route.dest_ip,
            prefix_len: route.prefix_len,
        };

        let gateway: &dyn Display = match &route.gateway {
            Some(g) => g,
            None => &"direct",
        };

        // route might have an interface that we don't have the record of.
        let interface = find_interface(route.interface_id, &ifs);

        let device: &dyn Display = match interface {
            Some(i) => &name_str(i),
            None => &route.interface_id,
        };

        let scope = if route.kind == RouteKind::Local {
            "host"
        } else if route.gateway.is_some() {
            "global"
        } else {
            "link"
        };

        let src = route
            .pref_src
            .or_else(|| interface.and_then(cidr).map(|c| c.addr));
        let src: &dyn Display = match &src {
            Some(s) => s,
            None => &"-",
        };

        println!(
            "{} {} {} {} {} {} {}",
            addr, gateway, device, route.metric, route.owner, scope, src
        );
    }
}

/// Parses `<dest>/<prefix> [via <gw>] dev <dev> [metric <n>] [src <addr>] [replace]`.
/// `metric`, `src` and `replace` are only accepted when `minimum` is unset (for `add`).
fn parse_route(rest: &mut impl Iterator<Item = &str>, minimum: bool) -> (RouteEntry, bool) {
    let Ipv4Cidr { addr, prefix_len } = Ipv4Cidr::from_str(next_arg(rest))
        .unwrap_or_else(|_| exit_with_msg("invalid destination address"));

    let mut gateway = None;
    let mut interface_id = None;
    let mut metric = DEFAULT_METRIC;
    let mut pref_src = None;
    let mut replace = false;

    while let Some(keyword) = rest.next() {
        match keyword {
            "via" => {
                gateway = Some(
                    Ipv4Addr::from_str(next_arg(rest))
                        .unwrap_or_else(|_| exit_with_msg("invalid gateway address")),
                )
            }
            "dev" => interface_id = Some(resolve_dev(next_arg(rest))),
            "metric" if !minimum => {
                metric = next_arg(rest)
                    .parse()
                    .unwrap_or_else(|_| exit_with_msg("invalid metric"))
            }
            "src" if !minimum => {
                pref_src = Some(
                    Ipv4Addr::from_str(next_arg(rest))
                        .unwrap_or_else(|_| exit_with_msg("invalid src address")),
                );
            }
            "replace" if !minimum => replace = true,
            _ => exit_with_msg(USAGE),
        }
    }

    let Some(interface_id) = interface_id else {
        exit_with_msg("dev is required");
    };

    let route = RouteEntry {
        dest_ip: addr,
        prefix_len,
        gateway,
        interface_id,
        metric,
        owner: RouteOwner::Static,
        kind: RouteKind::Unicast,
        pref_src,
    };

    (route, replace)
}

fn route_error(e: SysError) -> ! {
    match e {
        SysError::AlreadyExists => exit_with_msg("route exists (use replace)"),
        SysError::NotPermitted => exit_with_msg("kernel routes cannot be changed"),
        SysError::NoEntry => exit_with_msg("no such route or device"),
        SysError::InvalidArgument => exit_with_msg(
            "invalid route (host bits set in destination, or gateway not reachable on dev)",
        ),
        e => exit_with_msg(e),
    }
}

fn route_add_cmd(rest: &mut impl Iterator<Item = &str>) {
    let (route, replace) = parse_route(rest, false);

    let mut flags = 0;
    if replace {
        flags |= RouteFlag::ROUTE_REPLACE;
    }

    route_add(&route, flags).unwrap_or_else(|e| route_error(e));
}

fn route_remove_cmd(rest: &mut impl Iterator<Item = &str>) {
    let (route, _) = parse_route(rest, true);

    route_remove(&route).unwrap_or_else(|e| route_error(e));
}
