use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::net::eth::EthernetHeader;
use crate::net::socket::{RecvQueue, SockAddr};
use crate::net::{InterfaceId, NetError, interface};
use crate::param::NSOCKET;
use crate::spinlock::SpinLock;

#[derive(Debug)]
pub struct PacketSocket {
    pub interface: Option<InterfaceId>,
    pub ether_type: Option<u16>,
    queue: RecvQueue,
}

impl PacketSocket {
    /// Binds a socket. Interface, if given, must exist.
    pub fn bind(
        interface: Option<InterfaceId>,
        ether_type: Option<u16>,
    ) -> Result<Arc<Self>, NetError> {
        if let Some(id) = interface
            && interface::find_interface_by_id(id).is_none()
        {
            err!(NetError::InterfaceNotFound)
        }

        let mut table = PACKET_TABLE.lock();
        let socket = Arc::new(Self {
            interface,
            ether_type,
            queue: RecvQueue::new(),
        });
        try_log!(table.add(socket.clone()));
        Ok(socket)
    }

    pub fn close(self: &Arc<Self>) {
        {
            let mut table = PACKET_TABLE.lock();
            table.remove(self);
        }

        self.queue.close();
    }

    /// `frame` is a full Ethernet frame. Sent on `interface`, or the bound one if None.
    pub fn send(&self, interface: Option<InterfaceId>, frame: &[u8]) -> Result<(), NetError> {
        if frame.len() < 14 {
            err!(NetError::MalformedPacket);
        }

        if frame.len() > 1514 {
            err!(NetError::PacketTooLarge);
        }

        if self.queue.is_closed() {
            err!(NetError::BadSocket);
        }

        let Some(id) = interface.or(self.interface) else {
            err!(NetError::NotConfigured);
        };

        let Some(interface) = interface::find_interface_by_id(id) else {
            err!(NetError::InterfaceNotFound);
        };

        log!(interface.device.transmit(frame))
    }

    pub fn receive(&self) -> Result<(SockAddr, Box<[u8]>), NetError> {
        log!(self.queue.receive())
    }

    fn matches(&self, interface: InterfaceId, ether_type: u16) -> bool {
        if let Some(i) = self.interface
            && i != interface
        {
            return false;
        }

        if let Some(e) = self.ether_type
            && e != ether_type
        {
            return false;
        }

        true
    }

    pub fn set_receive_timeout(&self, ticks: usize) {
        self.queue.set_timeout(ticks);
    }
}

pub struct PacketTable {
    entries: [Option<Arc<PacketSocket>>; NSOCKET],
}

static PACKET_TABLE: SpinLock<PacketTable> = SpinLock::new(
    PacketTable {
        entries: [const { None }; NSOCKET],
    },
    "packet_sockets",
);

impl PacketTable {
    fn add(&mut self, socket: Arc<PacketSocket>) -> Result<(), NetError> {
        let Some(id) = self.entries.iter().position(|e| e.is_none()) else {
            err!(NetError::TableFull);
        };

        self.entries[id].replace(socket);

        Ok(())
    }

    fn remove(&mut self, socket: &Arc<PacketSocket>) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.as_ref().is_some_and(|s| Arc::ptr_eq(s, socket)))
        {
            *entry = None;
        }
    }
}

pub fn deliver_all(interface: InterfaceId, eth: &EthernetHeader, frame: &[u8]) {
    let mut sockets: [Option<Arc<PacketSocket>>; NSOCKET] = [const { None }; NSOCKET];

    let ether_type = eth.ether_type.get();

    // gather every matching socket under lock; then release before delivering
    {
        let table = PACKET_TABLE.lock();
        for (i, socket) in table.entries.iter().flatten().enumerate() {
            if socket.matches(interface, ether_type) {
                sockets[i] = Some(socket.clone());
            }
        }
    }

    for socket in sockets.iter().flatten() {
        socket.queue.deliver(
            SockAddr::Link {
                interface: Some(interface),
                ether_type: Some(ether_type),
                mac: eth.src,
            },
            Box::from(frame),
        );
    }
}
