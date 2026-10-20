use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::net::interface::{self, InterfaceConfig, InterfaceId, NetDevice};
use crate::net::{self, Ipv4Addr, Ipv4Config, MacAddr, NetError};
use crate::sync::OnceLock;

static LOOPBACK_ID: OnceLock<InterfaceId> = OnceLock::new();

pub fn interface_id() -> InterfaceId {
    *LOOPBACK_ID.get().expect("loopback to be setup")
}

/// A loopback network interface that allows the kernel to send packets to itself, bypassing the
/// physical layer.
struct LoopbackInterface {}

impl NetDevice for LoopbackInterface {
    fn transmit(&self, packet: &[u8]) -> Result<(), NetError> {
        // directly enqueue the packet for reception
        log!(net::enqueue_incoming(interface_id(), Box::from(packet)))
    }

    fn needs_arp(&self) -> bool {
        // does not require ARP since it only communicates with itself
        false
    }

    fn needs_dhcp(&self) -> bool {
        // does not require DHCP since it has a fixed IP address
        false
    }
}

pub fn init() {
    let device = Arc::new(LoopbackInterface {});

    let id = InterfaceId::next_interface_id();

    LOOPBACK_ID.initialize(|| Ok::<_, ()>(id));

    interface::register_interface(
        id,
        InterfaceConfig {
            name: "lo",
            mac: MacAddr::UNSPECIFIED,
            ipv4: Some(Ipv4Config {
                addr: Ipv4Addr([127, 0, 0, 1]),
                prefix_len: 8,
            }),
            is_up: true,
        },
        device,
    );
}
