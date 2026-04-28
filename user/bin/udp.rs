#![no_std]
#![no_main]

use user::*;

const USAGE: &str = "usage: net listen <port>
       net send <address> <port> [message]";

#[unsafe(no_mangle)]
fn main(args: Args) {
    if args.len() < 2 {
        exit_with_msg(USAGE);
    }

    match args.get_str(1).unwrap() {
        "listen" => {
            let Some(port) = args.get_str(2) else {
                exit_with_msg(USAGE);
            };

            let port = port.parse::<u16>().unwrap_or_else(|_| {
                exit_with_msg("invalid port number");
            });

            let fd = socket(port).expect("socket failed");

            let mut buf = [0u8; 1024];
            let mut src_ip = [0u8; 4];
            let mut src_port = 0u16;

            loop {
                let rc_len =
                    receive(fd, &mut buf, &mut src_ip, &mut src_port).expect("receive failed");

                println!(
                    "[{}:{}]: {}",
                    Ipv4Addr(src_ip),
                    src_port,
                    str::from_utf8(&buf[..rc_len]).unwrap_or("<invalid utf-8>")
                );
            }
        }

        "send" => {
            let Some(dest_ip) = args.get_str(2) else {
                exit_with_msg(USAGE);
            };

            let dest_ip = dest_ip.parse::<Ipv4Addr>().unwrap_or_else(|_| {
                exit_with_msg("net: invalid IP address");
            });

            let Some(dest_port) = args.get_str(3) else {
                exit_with_msg(USAGE);
            };

            let dest_port = dest_port.parse::<u16>().unwrap_or_else(|_| {
                exit_with_msg("net: invalid port number");
            });

            let fd = socket(0).expect("socket failed");

            // if message is provided, send it directly
            // otherwise read from stdin and send until EOF
            if let Some(message) = args.get_str(4) {
                send(fd, message.as_bytes(), &dest_ip.0, dest_port).expect("send failed");
            } else {
                let mut buf = [0u8; 1024];
                loop {
                    let len = read(Fd::STDIN, &mut buf).expect("read failed");
                    if len == 0 {
                        break;
                    }
                    send(
                        fd,
                        buf[..len].strip_suffix(b"\n").unwrap_or(&buf[..len]),
                        &dest_ip.0,
                        dest_port,
                    )
                    .expect("send failed");
                }
            };
        }

        _ => exit_with_msg(USAGE),
    }
}
