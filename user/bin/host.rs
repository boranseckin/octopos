#![no_std]
#![no_main]

use user::prelude::*;

use user::dns;

const USAGE: &str = "usage: host <name>";

#[unsafe(no_mangle)]
fn main(args: Args) {
    let Some(name) = args.get_str(1) else {
        exit_with_msg(USAGE);
    };

    if name.parse::<Ipv4Addr>().is_ok() {
        exit_with_msg("host: reverse lookups are not supported")
    }

    match dns::resolve(name) {
        Ok(addrs) => {
            for addr in addrs {
                println!("{name} has address {addr}");
            }
        }

        Err(e) => exit_with_msg(format_args!("host: {name}: {e}")),
    }
}
