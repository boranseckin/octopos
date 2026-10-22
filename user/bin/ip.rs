#![no_std]
#![no_main]

use core::fmt::Display;

use user::*;

const USAGE: &str = "usage: ip addr
       ip addr set <dev> <addr>/<prefix>
       ip addr clear <dev>
       ip route
       ip route add <dest>/<prefix> [via <gw>] dev <dev> [metric <n>] [src <addr>] [replace]
       ip route del <dest>/<prefix> [via <gw>] dev <dev>";

const MAX_INTERFACES: usize = 16;

#[unsafe(no_mangle)]
fn main(args: Args) {
    match (args.get_str(1), args.get_str(2)) {
        (Some("addr"), None) => addr_list_cmd(),
        // (Some("addr"), Some("set")) => addr_set_cmd(&mut rest),
        // (Some("addr"), Some("clear")) => addr_clear_cmd(&mut rest),
        (Some("route"), None) => route_list_cmd(),
        // (Some("route"), Some("add")) => route_add_cmd(&mut rest),
        // (Some("route"), Some("del")) => route_del_cmd(&mut rest),
        _ => exit_with_msg(USAGE),
    }
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
