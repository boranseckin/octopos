use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::net::{self, Ipv4Addr, MacAddr, NetError, NetInterface};

/// A loopback network interface that allows the kernel to send packets to itself, bypassing the
/// physical layer.
struct LoopbackInterface;

impl NetInterface for LoopbackInterface {
    fn name(&self) -> &'static str {
        "lo"
    }

    fn mac(&self) -> MacAddr {
        MacAddr::UNSPECIFIED
    }

    fn ip(&self) -> Option<Ipv4Addr> {
        Some(Ipv4Addr([127, 0, 0, 1]))
    }

    fn transmit(&self, packet: &[u8]) -> Result<(), NetError> {
        // directly enqueue the packet for reception
        log!(net::enqueue_incoming(Box::from(packet)))
    }

    fn needs_arp(&self) -> bool {
        // does not require ARP since it only communicates with itself
        false
    }
}

pub fn init() {
    net::register_interface(Arc::new(LoopbackInterface));
}
