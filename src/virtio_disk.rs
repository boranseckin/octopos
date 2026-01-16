// virtio device definitions.
// for both the mmio interface, and virtio descriptors.
// only tested with qemu.
//
// the virtio spec:
// https://docs.oasis-open.org/virtio/virtio/v1.1/virtio-v1.1.pdf

use core::ptr;

use crate::memlayout::VIRTIO0;
use crate::spinlock::SpinLock;

// virtio mmio control registers, mapped starting at 0x10001000.
// from qemu virtio_mmio.h
const VIRTIO_MMIO_MAGIC_VALUE: u32 = 0x000; // 0x74726976
const VIRTIO_MMIO_VERSION: u32 = 0x004; // version; should be 2
const VIRTIO_MMIO_DEVICE_ID: u32 = 0x008; // device type; 1 is net, 2 is disk
const VIRTIO_MMIO_VENDOR_ID: u32 = 0x00c; // 0x554d4551
const VIRTIO_MMIO_DEVICE_FEATURES: u32 = 0x010;
const VIRTIO_MMIO_DRIVER_FEATURES: u32 = 0x020;
const VIRTIO_MMIO_QUEUE_SEL: u32 = 0x030; // select queue, write-only
const VIRTIO_MMIO_QUEUE_NUM_MAX: u32 = 0x034; // max size of current queue, read-only
const VIRTIO_MMIO_QUEUE_NUM: u32 = 0x038; // size of current queue, write-only
const VIRTIO_MMIO_QUEUE_READY: u32 = 0x044; // ready bit
const VIRTIO_MMIO_QUEUE_NOTIFY: u32 = 0x050; // write-only
const VIRTIO_MMIO_INTERRUPT_STATUS: u32 = 0x060; // read-only
const VIRTIO_MMIO_INTERRUPT_ACK: u32 = 0x064; // write-only
const VIRTIO_MMIO_STATUS: u32 = 0x070; // read/write
const VIRTIO_MMIO_QUEUE_DESC_LOW: u32 = 0x080; // physical address for descriptor table, write-only
const VIRTIO_MMIO_QUEUE_DESC_HIGH: u32 = 0x084;
const VIRTIO_MMIO_DRIVER_DESC_LOW: u32 = 0x090; // physical address for available ring, write-only
const VIRTIO_MMIO_DRIVER_DESC_HIGH: u32 = 0x094;
const VIRTIO_MMIO_DEVICE_DESC_LOW: u32 = 0x0a0; // physical address for used ring, write-only
const VIRTIO_MMIO_DEVICE_DESC_HIGH: u32 = 0x0a4;

// status register bits, from qemu virtio_config.h
const VIRTIO_CONFIG_S_ACKNOWLEDGE: u32 = 1;
const VIRTIO_CONFIG_S_DRIVER: u32 = 2;
const VIRTIO_CONFIG_S_DRIVER_OK: u32 = 4;
const VIRTIO_CONFIG_S_FEATURES_OK: u32 = 8;

// device feature bits
const VIRTIO_BLK_F_RO: u32 = 5; // disk is read only
const VIRTIO_BLK_F_SCSI: u32 = 7; // Supports scsi command passthru
const VIRTIO_BLK_F_CONFIG_WCE: u32 = 11; // Writeback mode available in config
const VIRTIO_BLK_F_MQ: u32 = 12; // support more than one vq
const VIRTIO_F_ANY_LAYOUT: u32 = 27;
const VIRTIO_RING_F_INDIRECT_DESC: u32 = 28;
const VIRTIO_RING_F_EVENT_IDX: u32 = 29;

const NUM: usize = 8;

const fn R(r: usize) -> usize {
    VIRTIO0 + r
}

/// A single descriptor from the spec
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

/// The (entire) avail ring, from the spec
#[repr(C)]
#[derive(Debug, Clone)]
struct VirtqAvail {
    flags: u16,       // always zero
    idx: u16,         // driver will write ring[idx] next
    ring: [u16; NUM], // descriptor numbers of chain heads
    unused: u16,
}

/// One entry in the "used" ring, with which the device tells the driver about completed requests
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VirtqUsedElem {
    id: u32, // index of start of completed descriptor chain
    len: u32,
}

pub static VIRTIO_DISK: SpinLock<Disk> = SpinLock::new(Disk::new(), "virtio_disk");

#[repr(C)]
#[derive(Debug, Clone)]
struct VirtqUsed {
    flags: u16, // always zero
    idx: u16,   // device increments when it adds a ring[] entry
    ring: [VirtqUsedElem; NUM],
}

// these are specific to virtio block devices, e.g. disks,
// described in Section 5.2 of the spec.

const VIRTIO_BLK_T_IN: usize = 0; // read the disk
const VIRTIO_BLK_T_OUT: usize = 1; // write the disk

/// The format of the first descriptor in a disk request.
/// To be followed by two more descriptors containing the block, and a one-byte status.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct BlockReq {
    r#type: u32,
    reserved: u32,
    sector: u32,
}

pub struct Disk {
    /// A set (not a ring) of DMA descriptors, with which the driver tells the device where to read
    /// and write individual disk operations. there are NUM descriptors. Most commands consist of a
    /// "chain" (a linked list) of a couple of these descriptors.
    desc: VirtqDesc,

    /// A ring in which the driver writes descriptor numbers that the driver would like the device
    /// to process. It only includes the head descriptor of each chain. The ring has NUM elements.
    avail: VirtqAvail,

    /// A ring in which the device writes descriptor numbers that the device has finished processing
    /// (just the head of each chain). There are NUM used ring entries.
    used: VirtqUsed,

    /// Is a descriptor free?
    free: [bool; NUM],
}

impl Disk {
    pub const fn new() -> Self {
        Disk {
            desc: VirtqDesc {
                addr: 0,
                len: 0,
                flags: 0,
                next: 0,
            },
            avail: VirtqAvail {
                flags: 0,
                idx: 0,
                ring: [0; NUM],
                unused: 0,
            },
            used: VirtqUsed {
                flags: 0,
                idx: 0,
                ring: [VirtqUsedElem { id: 0, len: 0 }; NUM],
            },
            free: [true; NUM],
        }
    }

    /// Read a 4 bytes from the given VIRTIO register.
    fn read(&self, reg: u32) -> u32 {
        // Safety: reading from memory-mapped VIRTIO register
        unsafe { ptr::read_volatile((VIRTIO0 + reg as usize) as *const u32) }
    }

    /// Write 4 bytes to the given VIRTIO0 register.
    fn write(&mut self, reg: u32, value: u32) {
        // Safety: writing to memory-mapped UART register
        unsafe { ptr::write_volatile((VIRTIO0 + reg as usize) as *mut u32, value) }
    }
}

pub unsafe fn init() {
    let mut disk = VIRTIO_DISK.lock();
    let mut status = 0;

    assert!(
        disk.read(VIRTIO_MMIO_MAGIC_VALUE) == 0x74726976
            && disk.read(VIRTIO_MMIO_VERSION) == 2
            && disk.read(VIRTIO_MMIO_DEVICE_ID) == 2
            && disk.read(VIRTIO_MMIO_VENDOR_ID) == 0x554d4551,
        "could not find virtio disk"
    );

    // reset device
    disk.write(VIRTIO_MMIO_STATUS, status);

    // set ACKNOWLEDGE status bit
    status |= VIRTIO_CONFIG_S_ACKNOWLEDGE;
    disk.write(VIRTIO_MMIO_STATUS, status);

    // set DRIVER status bit
    status |= VIRTIO_CONFIG_S_DRIVER;
    disk.write(VIRTIO_MMIO_STATUS, status);

    // negotiate features
    let mut features = disk.read(VIRTIO_MMIO_DEVICE_FEATURES);
    features &= !(1 << VIRTIO_BLK_F_RO);
    features &= !(1 << VIRTIO_BLK_F_SCSI);
    features &= !(1 << VIRTIO_BLK_F_CONFIG_WCE);
    features &= !(1 << VIRTIO_BLK_F_MQ);
    features &= !(1 << VIRTIO_F_ANY_LAYOUT);
    features &= !(1 << VIRTIO_RING_F_EVENT_IDX);
    features &= !(1 << VIRTIO_RING_F_INDIRECT_DESC);
    disk.write(VIRTIO_MMIO_DRIVER_FEATURES, features);

    // tell device that feature negotiation is complete
    status |= VIRTIO_CONFIG_S_FEATURES_OK;
    disk.write(VIRTIO_MMIO_STATUS, status);

    // re-read status to ensure FEATURES_OK is set
    status = disk.read(VIRTIO_MMIO_STATUS);
    assert_ne!(
        status & VIRTIO_CONFIG_S_FEATURES_OK,
        0,
        "virtio disk features negotiation failed"
    );

    // initialize queue 0
    disk.write(VIRTIO_MMIO_QUEUE_SEL, 0);

    // ensure queue 0 is not in use
    assert_eq!(
        disk.read(VIRTIO_MMIO_QUEUE_READY),
        0,
        "virtio disk queue 0 in use"
    );

    // check maximum queue size
    let max = disk.read(VIRTIO_MMIO_QUEUE_NUM_MAX);
    assert_ne!(max, 0, "virtio disk has no queue 0");
    assert!(max as usize > NUM, "virito disk max queue too short");

    // set queue size
    disk.write(VIRTIO_MMIO_QUEUE_NUM, NUM.try_into().unwrap());

    // write physical addresses
    let desc_addr = &disk.desc as *const _ as usize;
    disk.write(VIRTIO_MMIO_QUEUE_DESC_LOW, desc_addr as u32);
    disk.write(VIRTIO_MMIO_QUEUE_DESC_HIGH, (desc_addr >> 32) as u32);

    let avail_addr = &disk.avail as *const _ as usize;
    disk.write(VIRTIO_MMIO_DRIVER_DESC_LOW, avail_addr as u32);
    disk.write(VIRTIO_MMIO_DRIVER_DESC_HIGH, (avail_addr >> 32) as u32);

    let used_addr = &disk.used as *const _ as usize;
    disk.write(VIRTIO_MMIO_DEVICE_DESC_LOW, used_addr as u32);
    disk.write(VIRTIO_MMIO_DEVICE_DESC_HIGH, (used_addr >> 32) as u32);

    // queue is ready
    disk.write(VIRTIO_MMIO_QUEUE_READY, 1);

    // tell device we are completely ready
    status |= VIRTIO_CONFIG_S_DRIVER_OK;
    disk.write(VIRTIO_MMIO_STATUS, status);
}
