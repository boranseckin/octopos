use core::ptr;
use core::slice;
use core::sync::atomic::{AtomicUsize, Ordering};

use alloc::boxed::Box;
use alloc::sync::Arc;

use crate::net::interface::{self, InterfaceConfig, InterfaceId, NetDevice};
use crate::net::{self, Ipv4Addr, Ipv4Config, MacAddr, NetError};
use crate::spinlock::SpinLock;

const TX_RING_SIZE: usize = 16;
const RX_RING_SIZE: usize = 16;
const TX_BUF_SIZE: usize = 2048;
const RX_BUF_SIZE: usize = 2048;

const E1000_MMIO_CTRL: usize = 0x0000;
const E1000_MMIO_ICR: usize = 0x00C0;
const E1000_MMIO_IMS: usize = 0x00D0;
const E1000_MMIO_IMC: usize = 0x00D8;

const E1000_MMIO_RCTL: usize = 0x0100;
const E1000_MMIO_TCTL: usize = 0x0400;
const E1000_MMIO_TIPG: usize = 0x0410;

const E1000_MMIO_RDBAL: usize = 0x2800;
const E1000_MMIO_RDBAH: usize = 0x2804;
const E1000_MMIO_RDLEN: usize = 0x2808;
const E1000_MMIO_RDH: usize = 0x2810;
const E1000_MMIO_RDT: usize = 0x2818;

const E1000_MMIO_TDBAL: usize = 0x3800;
const E1000_MMIO_TDBAH: usize = 0x3804;
const E1000_MMIO_TDLEN: usize = 0x3808;
const E1000_MMIO_TDH: usize = 0x3810;
const E1000_MMIO_TDT: usize = 0x3818;

const E1000_MMIO_MTA: usize = 0x5200;
const E1000_MMIO_RAL: usize = 0x5400;
const E1000_MMIO_RAH: usize = 0x5404;

/// To be set by `pci::init()`, if the device is found
pub static E1000_BASE: AtomicUsize = AtomicUsize::new(0);

static E1000: SpinLock<E1000> = SpinLock::new(E1000::new(), "e1000");

/// Transmit Descriptor (TxDesc) structure as defined in the e1000 datasheet.
/// Each descriptor is 16 bytes in size and must be aligned to a 16-byte boundary.
///
/// Implemented according to 3.3.3 Legacy Transmit Descriptor Format.
#[repr(C, align(16))]
#[derive(Debug, Clone, Default)]
struct TxDesc {
    address: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

impl TxDesc {
    const fn new() -> Self {
        Self {
            address: 0,
            length: 0,
            cso: 0,
            cmd: 0,
            status: 0,
            css: 0,
            special: 0,
        }
    }
}

/// Receive Descriptor (RxDesc) structure as defined in the e1000 datasheet.
/// Each descriptor is 16 bytes in size and must be aligned to a 16-byte boundary.
///
/// Implemented according to 3.2.3 Receive Descriptor Format.
#[repr(C, align(16))]
#[derive(Debug, Clone, Default)]
struct RxDesc {
    address: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

impl RxDesc {
    const fn new() -> Self {
        Self {
            address: 0,
            length: 0,
            status: 0,
            special: 0,
            checksum: 0,
            errors: 0,
        }
    }
}

/// E1000 driver state.
///
/// Includes the base MMIO address, transmit and receive descriptor rings, and indices for tracking
/// the next available descriptors for transmission and reception.
#[derive(Debug)]
struct E1000 {
    base: usize,
    tx_rings: [TxDesc; TX_RING_SIZE],
    rx_rings: [RxDesc; RX_RING_SIZE],
    tx_index: usize,
    rx_index: usize,
    interface_id: Option<InterfaceId>,
}

impl E1000 {
    const fn new() -> Self {
        Self {
            base: 0,
            tx_rings: [const { TxDesc::new() }; TX_RING_SIZE],
            rx_rings: [const { RxDesc::new() }; RX_RING_SIZE],
            tx_index: 0,
            rx_index: 0,
            interface_id: None,
        }
    }

    fn read_reg(&self, offset: usize) -> u32 {
        unsafe { ptr::read_volatile((self.base as *const u32).add(offset / 4)) }
    }

    fn write_reg(&mut self, offset: usize, value: u32) {
        unsafe { ptr::write_volatile((self.base as *mut u32).add(offset / 4), value) }
    }

    fn set_mask(&mut self, offset: usize, mask: u32) {
        let value = self.read_reg(offset);
        self.write_reg(offset, value | mask);
    }
}

pub struct E1000Interface;

impl NetDevice for E1000Interface {
    /// Transmits a packet by copying the provided buffer into the next available transmit
    /// descriptor and updating the transmit tail pointer to notify the hardware.
    ///
    /// Returns an error if the buffer size exceeds the transmit buffer size or if there are no
    /// available transmit descriptors (i.e., the hardware has not yet processed the previous
    /// descriptors).
    fn transmit(&self, packet: &[u8]) -> Result<(), NetError> {
        if packet.len() >= TX_BUF_SIZE {
            err!(NetError::PacketTooLarge);
        }

        let mut state = E1000.lock();

        let index = state.tx_index;
        let desc = &mut state.tx_rings[index];

        // check if bit 0 of status (DD) is unset; not available to use
        if desc.status & 1 == 0 {
            err!(NetError::ResourceUnavailable);
        }

        // copy buf into tx desc
        //
        // Safety: We assume the caller has provided a valid buffer and that the e1000 device is
        // properly initialized. The destination address is derived from the tx descriptor, which
        // should have been set up during initialization to point to a valid transmit buffer.
        // We also ensure that the length of the buffer does not exceed the size of the transmit
        // buffer.
        unsafe {
            (desc.address as *mut u8).copy_from_nonoverlapping(packet.as_ptr(), packet.len())
        };
        desc.length = packet.len() as u16;

        // set cmd EOP, IFCS, RS
        desc.cmd = (1) | (1 << 1) | (1 << 3);

        // clear status
        desc.status = 0;

        // update tx index in hardware and software
        let index = (index + 1) % TX_RING_SIZE;
        state.tx_index = index;
        state.write_reg(E1000_MMIO_TDT, index as u32);

        Ok(())
    }
}

/// Handles an interrupt from the e1000 device by checking the Interrupt Cause Read (ICR) register
/// to determine the cause of the interrupt. If the interrupt was caused by a received packet
/// (RXT0), the function disables further RXT0 interrupts until the packet is processed.
///
/// It checks the next receive descriptor for a completed packet, copyies the data into an owned
/// buffer, updates the receive tail pointer to give ownership back to the hardware, and re-enables
/// receive interrupts.
pub fn handle_interrupt() {
    let mut state = E1000.lock();

    let Some(interface_id) = state.interface_id else {
        // should never happen since we set the interface ID during initialization before enabling
        // interrupts, but just in case, we return early if the interface ID is not set.
        return;
    };

    // reading ICR clears the interrupt
    let icr = state.read_reg(E1000_MMIO_ICR);

    // check if ICR bit 7 (RXT0) is set
    if icr & (1 << 7) == 0 {
        return;
    }

    // disable RXT0 interrupts until receive is done
    state.set_mask(E1000_MMIO_IMC, 1 << 7);

    loop {
        let index = state.rx_index;
        let status = unsafe { ptr::read_volatile(&state.rx_rings[index].status) };

        if status & 1 == 0 {
            // no more packets found
            break;
        }

        let desc = &state.rx_rings[index];
        let len = desc.length as usize;
        let buf_ptr = desc.address as *const u8;
        let data = unsafe { slice::from_raw_parts(buf_ptr, len) };

        // copy the package to an owned Box so that when we give this memory back to the
        // hardware, our local copy will not be modified.
        let data = Box::from(data);

        // update RDT, which now gives the hardware ownership of the descriptor back and allows
        // it to write the next packet into it.
        state.rx_rings[index].status = 0;
        state.write_reg(E1000_MMIO_RDT, index as u32);
        state.rx_index = (index + 1) % RX_RING_SIZE;

        // drop the lock to keep the interrupt-disabled window short.
        // enqueuing and waking the network thread do not need access to the descriptor ring.
        drop(state);

        // enqueue the packet for processing by the network stack
        // ignore the result since if the channel is full, the packet will just be dropped
        let _ = log!(net::enqueue_incoming(interface_id, data));

        // re-acquire lock for next iteration of loop
        state = E1000.lock();
    }

    // re-enable rx interrupts (RXT0)
    state.set_mask(E1000_MMIO_IMS, 1 << 7);
}

/// Initializes the e1000 driver by mapping the MMIO region, resetting the device, setting up
/// receive and transmit descriptor rings, configuring the MAC address, and enabling interrupts.
///
/// Follows chapter 14 of https://pdos.csail.mit.edu/6.828/2025/readings/8254x_GBe_SDM.pdf.
///
/// # Safety
///
/// This function performs raw pointer dereferencing to access the device's MMIO region and assumes
/// that the e1000 device is present and properly configured by `pci::init()`.
pub unsafe fn init() {
    let base = E1000_BASE.load(Ordering::SeqCst);

    if base == 0 {
        println!("e1000 not found");
        return;
    }

    let mut state = E1000.lock();

    state.base = base;

    // set RST bit to trigger a full reset
    // documentation suggests a 1us sleep after reset, but we ignore it
    state.set_mask(E1000_MMIO_CTRL, 1 << 26);

    // allocate receive buffers
    // each buffer is 2 KB in size by default set in RCTL.BSIZE.
    for rx_ring in &mut state.rx_rings {
        unsafe {
            rx_ring.address = Box::into_raw(
                Box::<[u8; RX_BUF_SIZE]>::try_new_zeroed()
                    .expect("failed to allocate rx buffer")
                    .assume_init(),
            ) as u64;
        }
    }

    // set RAL/RAH to MAC 52:54:00:12:34:56 and address valid bit RAH[31]
    state.write_reg(E1000_MMIO_RAL, 0x12005452);
    state.write_reg(E1000_MMIO_RAH, 0x80005634);

    // set MTA to 0 for all 128 entries
    for i in 0..128 {
        state.write_reg(E1000_MMIO_MTA + (i * 4), 0);
    }

    // enable interrupts: IMS = RXT0 (bit7) | LSC (bit2)
    state.set_mask(E1000_MMIO_IMS, (1 << 7) | (1 << 2));

    // set RDBAL/RDBAH to PA of rx_ring, RDLEN, RDH=0, RDT=RX_RING_SIZE-1
    let rx_base = state.rx_rings.as_ptr() as u64;
    state.write_reg(E1000_MMIO_RDBAL, (rx_base & 0xFFFF_FFFF) as u32);
    state.write_reg(E1000_MMIO_RDBAH, (rx_base >> 32) as u32);
    state.write_reg(
        E1000_MMIO_RDLEN,
        (size_of::<RxDesc>() * RX_RING_SIZE) as u32,
    );
    state.write_reg(E1000_MMIO_RDH, 0);
    state.write_reg(E1000_MMIO_RDT, RX_RING_SIZE as u32 - 1);

    // set RCTL: EN=bit1, SBP=bit2, UPE=bit3, MPE=bit4, LBM=0, RDMTS=0, BAM=bit15, BSIZE=2048
    state.set_mask(
        E1000_MMIO_RCTL,
        (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 15),
    );

    // allocate transmit buffers
    // each buffer is 2 KB in size by default set in TCTL.BSIZE.
    for tx_ring in &mut state.tx_rings {
        unsafe {
            // set DD bit to signify it is free
            tx_ring.status = 1;
            tx_ring.address = Box::into_raw(
                Box::<[u8; TX_BUF_SIZE]>::try_new_zeroed()
                    .expect("failed to allocate tx buffer")
                    .assume_init(),
            ) as u64;
        }
    }

    // set TDBAL/TDBAH to PA of tx_ring, TDLEN, TDH=0, TDT=0
    let tx_base = state.tx_rings.as_ptr() as u64;
    state.write_reg(E1000_MMIO_TDBAL, (tx_base & 0xFFFF_FFFF) as u32);
    state.write_reg(E1000_MMIO_TDBAH, (tx_base >> 32) as u32);
    state.write_reg(
        E1000_MMIO_TDLEN,
        (size_of::<TxDesc>() * TX_RING_SIZE) as u32,
    );
    state.write_reg(E1000_MMIO_TDH, 0);
    state.write_reg(E1000_MMIO_TDT, 0);

    // set TCTL: EN=bit1, PSP=bit3, CT=0x10<<4, COLD=0x40<<12
    // Collision Threshold (CT) is don't care in full-duplex but we are setting it regardless.
    state.set_mask(
        E1000_MMIO_TCTL,
        (1 << 1) | (1 << 3) | (0x10 << 4) | (0x40 << 12),
    );

    // set TIPG: IPGT=10, IPGR1=8, IPGR2=6
    state.set_mask(E1000_MMIO_TIPG, (10) | (8 << 10) | (6 << 20));

    // register the network interface with the network stack
    let interface_id = interface::register_interface(
        InterfaceConfig {
            name: "e1000",
            mac: MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]),
            ipv4: Some(Ipv4Config {
                addr: Ipv4Addr([192, 168, 10, 2]),
                prefix_len: 24,
            }),
            is_up: true,
        },
        Arc::new(E1000Interface),
    );

    state.interface_id = Some(interface_id);

    println!("e1000 init");
}
