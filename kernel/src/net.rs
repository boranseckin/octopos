use core::ptr;

use alloc::boxed::Box;

use crate::e1000;
use crate::proc;

const ETH_TYPE_ARP: u16 = 0x0806;
const ETH_TYPE_IP4: u16 = 0x0800;

const ARP_HTYPE_ETH: u16 = 0x1;
const ARP_OP_REQ: u16 = 0x1;
const ARP_OP_RES: u16 = 0x2;

/// Ethernet Frame Header
/// https://en.wikipedia.org/wiki/Ethernet_frame#Ethernet_II
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct EthernetHeader {
    /// Destination MAC Address
    dst: [u8; 6],
    /// Source MAC Address
    src: [u8; 6],
    /// Type
    eth_type: u16,
}

/// Address Resolution Protocol (ARP) Packet
/// https://datatracker.ietf.org/doc/html/rfc826
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
struct ArpPacket {
    /// Hardware Type
    htype: u16,
    /// Protocol Type
    ptype: u16,
    /// Hardware Address Length
    hlen: u8,
    /// Protocol Address Length
    plen: u8,
    /// Operation
    op: u16,
    /// Sender Hardware Address
    sha: [u8; 6],
    /// Sender Protocol Address
    spa: [u8; 4],
    /// Target Hardware Address
    tha: [u8; 6],
    /// Target Protocol Address
    tpa: [u8; 4],
}

unsafe trait NetworkHeader: Sized {
    fn from_bytes(data: &[u8]) -> Option<&Self> {
        if data.len() < size_of::<Self>() {
            return None;
        }

        // # Safety: the data is long enough, and the alignment is handled by repr(packed).
        Some(unsafe { &*(data.as_ptr() as *const Self) })
    }
}

unsafe impl NetworkHeader for EthernetHeader {}
unsafe impl NetworkHeader for ArpPacket {}

fn handle_arp(packet: &[u8]) {
    let Some(arp) = ArpPacket::from_bytes(packet) else {
        return;
    };

    let op = u16::from_be(arp.op);
    let htype = u16::from_be(arp.htype);
    let ptype = u16::from_be(arp.ptype);

    if op == ARP_OP_REQ && htype == ARP_HTYPE_ETH && ptype == ETH_TYPE_IP4 {
        if arp.tpa == [192, 168, 10, 2] {
            send_arp_reply(arp);
        }
    }
}

fn send_arp_reply(request: &ArpPacket) {
    let mut reply_packet = [0u8; 42]; // 14 + 28

    let eth_header = EthernetHeader {
        dst: request.sha,
        src: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        eth_type: ETH_TYPE_ARP.to_be(),
    };

    let arp_reply = ArpPacket {
        htype: ARP_HTYPE_ETH.to_be(),
        ptype: ETH_TYPE_IP4.to_be(),
        hlen: 6,
        plen: 4,
        op: ARP_OP_RES.to_be(),
        sha: [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        spa: [192, 168, 10, 2],
        tha: request.sha,
        tpa: request.spa,
    };

    // Copy structs into the buffer
    unsafe {
        ptr::copy_nonoverlapping(
            &eth_header as *const _ as *const u8,
            reply_packet.as_mut_ptr(),
            14,
        );
        ptr::copy_nonoverlapping(
            &arp_reply as *const _ as *const u8,
            reply_packet.as_mut_ptr().add(14),
            28,
        );
    }

    e1000::transmit(&reply_packet).unwrap();
}

fn rx(packet: Box<[u8]>) {
    let Some(eth) = EthernetHeader::from_bytes(&packet) else {
        return;
    };

    let eth_type = u16::from_be(eth.eth_type);

    match eth_type {
        ETH_TYPE_ARP => handle_arp(&packet[size_of::<EthernetHeader>()..]),
        _ => {}
    }
}

fn thread() {
    loop {
        let data = e1000::receive();
        rx(data);
    }
}

pub fn init() {
    proc::spawn_kernel_thread(thread, "net");
}
