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

            let fd =
                socket(SocketType::Udp, SockAddr::any_inet_with_port(port)).expect("socket failed");

            let mut buf = [0u8; 1024];

            loop {
                let (rc_len, rc_addr) = receive(fd, &mut buf).expect("receive failed");
                let SockAddr::Inet { ip, port } = rc_addr;

                println!(
                    "[{}:{}]: {}",
                    ip,
                    port,
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

            let dest = SockAddr::inet(dest_ip, dest_port);

            let fd = socket(SocketType::Udp, SockAddr::any_inet()).expect("socket failed");

            // if message is provided, send it directly
            // otherwise read from stdin and send until EOF
            if let Some(message) = args.get_str(4) {
                send(fd, message.as_bytes(), dest).expect("send failed");
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
                        dest,
                    )
                    .expect("send failed");
                }
            };
        }

        _ => exit_with_msg(USAGE),
    }
}
