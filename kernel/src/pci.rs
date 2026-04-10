use core::ptr;
use core::sync::atomic::Ordering;

use crate::e1000::E1000_BASE;
use crate::memlayout::{PCI_ECAM, PCI_MMIO};
use crate::spinlock::SpinLock;

static MMIO_NEXT: SpinLock<usize> = SpinLock::new(PCI_MMIO, "pci_mmio");

/// Calculates the offset into the PCI ECAM space for a given bus/device/function.
///
/// https://docs.amd.com/r/en-US/pg344-pcie-dma-versal/Enhanced-Configuration-Access-Memory-Map
///
/// The PCI ECAM (Enhanced Configuration Access Mechanism) space is organized as follows:
/// - Bits 27:20 represent the bus number (256 buses)
/// - Bits 19:15 represent the device number (32 devices)
/// - Bits 14:12 represent the function number (8 functions)
const fn get_ecam_offset(bus: usize, device: usize, function: usize) -> usize {
    // https://docs.amd.com/r/en-US/pg344-pcie-dma-versal/Enhanced-Configuration-Access-Memory-Map
    // 14:12 - Function Number (8 functions)
    // 19:15 - Device Number (32 devices)
    // 27:20 - Bus Number (256 buses)
    PCI_ECAM + (bus << 20) + (device << 15) + (function << 12)
}

/// Sets up a device's BAR0 and returns the mapped base address, or None if the BAR is not usable.
///
/// # Safety
///
/// This function performs raw pointer dereferencing to access the device's BAR0. It assumes that
/// the caller has already verified that the device exists and that `bar0_ptr` points to a valid
/// BAR0 register in the device's PCI configuration space. It also assumes that the system's MMIO
/// region for PCI BARs starts at `PCI_MMIO`.
unsafe fn setup_bar0(bar0_ptr: *mut u32) -> Option<u64> {
    // https://wiki.osdev.org/PCI#Base_Address_Registers

    unsafe {
        let bar0_lo_ptr = bar0_ptr;
        let bar0_hi_ptr = bar0_ptr.add(1);

        // write a mask and read back the value
        ptr::write_volatile(bar0_lo_ptr, 0xFFFF_FFFF);
        let bar0_lo = ptr::read_volatile(bar0_lo_ptr);

        // if read back is 0 (unimplemented) or bit-0 is set (I/O space BAR), bail.
        // we only handle MMIO space BAR setup, bit-0 is unset.
        if bar0_lo == 0 || bar0_lo & 1 == 1 {
            return None;
        }

        // bits 2:1 are 0b00 for 32-bit and 0b10 for 64-bit (0b01 is reserved)
        let is_64bit = bar0_lo & 0x6 == 0x4;

        let bar0_hi = if is_64bit {
            ptr::write_volatile(bar0_hi_ptr, 0xFFFF_FFFF);
            ptr::read_volatile(bar0_hi_ptr)
        } else {
            0
        };

        // combine both parts, no change if 32-bit address
        let bar0: u64 = bar0_lo as u64 | ((bar0_hi as u64) << 32);

        // first 4 bits are not used for addressing
        let mask = bar0 & !0b1111;

        let mut cursor = MMIO_NEXT.lock();

        // align the cursor up to the size of the BAR, since PCI requires natural alignment
        //
        // mask is always a power of 2, so !mask is a bitmask of the lower bits that must be zero
        // for alignment
        //
        // adding !mask to cursor ensures that if cursor is already aligned, it stays the same,
        // otherwise it moves up to the next aligned address
        //
        // then we clear the lower bits by anding with the mask, which gives us the aligned address
        let aligned = (*cursor as u64 + !mask) & mask;

        // if 32-bit, we need to complement mask in 32-bits
        let size = if is_64bit {
            !mask + 1
        } else {
            (!(mask as u32) as u64) + 1
        };

        *cursor = (aligned + size) as usize;

        // write the cursor to the BAR
        ptr::write_volatile(bar0_lo_ptr, (aligned & 0xFFFF_FFFF) as u32);
        ptr::write_volatile(bar0_hi_ptr, (aligned >> 32) as u32);

        println!("\tmapped base=0x{:08X}, size=0x{:08X}", aligned, size);

        Some(aligned)
    }
}

/// Scans the PCI bus for devices, sets up their BARs, and enables bus mastering and memory space
/// access. For simplicity, we only support single-function devices (function 0).
///
/// # Safety
///
/// This function performs raw pointer dereferencing to access PCI configuration space and device
/// BARs. It assumes that the system's PCI configuration space is memory-mapped at `PCI_ECAM` and
/// that the MMIO region for device BARs starts at `PCI_MMIO`.
pub unsafe fn init() {
    println!("");

    for bus in 0..256 {
        for device in 0..32 {
            unsafe {
                // single-function pci, check function 0 only
                let addr = get_ecam_offset(bus, device, 0) as *const u32;

                // get vendor id (bits 15:0) and device id (bits 31:16)
                // https://wiki.osdev.org/PCI#Common_Header_Fields
                let vendor_id = ptr::read_volatile(addr) as u16;
                let device_id = (ptr::read_volatile(addr) >> 16) as u16;

                if vendor_id == 0xFFFF {
                    continue;
                }

                println!(
                    "device: bus={}, device={}, vendor_id=0x{:04X}, device_id=0x{:04X}",
                    bus, device, vendor_id, device_id
                );

                let bar0_ptr = addr.add(4) as *mut u32;
                let Some(bar_addr) = setup_bar0(bar0_ptr) else {
                    continue;
                };

                // set command register bit 2 (bus master) and bit 1 (memory space)
                // https://wiki.osdev.org/PCI#Command_Register
                let command_reg = addr.add(1) as *mut u16;
                let mut cmd = ptr::read_volatile(command_reg);
                cmd |= (1 << 2) + (1 << 1);
                ptr::write_volatile(command_reg, cmd);

                // save E1000's BAR address
                if vendor_id == 0x8086 && device_id == 0x100E {
                    E1000_BASE.store(bar_addr as usize, Ordering::SeqCst);
                }
            }
        }
    }

    println!("\npci  init");
}
