use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::net::{
    self, InterfaceConfig, InterfaceId, Ipv4Addr, Ipv4Config, MacAddr, NetDevice, NetError,
};
use crate::sync::OnceLock;

/// A loopback network interface that allows the kernel to send packets to itself, bypassing the
/// physical layer.
struct LoopbackInterface {
    interface_id: OnceLock<InterfaceId>,
}

impl NetDevice for LoopbackInterface {
    fn transmit(&self, packet: &[u8]) -> Result<(), NetError> {
        // directly enqueue the packet for reception
        log!(net::enqueue_incoming(
            *self
                .interface_id
                .get()
                .expect("loopback interface ID not set"),
            Box::from(packet)
        ))
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
    let device = Arc::new(LoopbackInterface {
        interface_id: OnceLock::new(),
    });

    let interface_id = net::register_interface(
        InterfaceConfig {
            name: "lo",
            mac: MacAddr::UNSPECIFIED,
            ipv4: Some(Ipv4Config {
                addr: Ipv4Addr([127, 0, 0, 1]),
                prefix_len: 8,
            }),
            is_up: true,
        },
        device.clone(),
    );

    device.interface_id.initialize(|| Ok::<_, ()>(interface_id));
}
