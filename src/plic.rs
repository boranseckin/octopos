// RISCV Platfform Level Interrupt Controller (PLIC)

use crate::{
    memlayout::{PLIC, PLIC_SCLAIM, PLIC_SENABLE, PLIC_SPRIORITY, UART0_IRQ, VIRTIO0_IRQ},
    proc::{CPUS, Cpu, Cpus},
};

pub fn init() {
    // set desired IRQ priorities non-zero (otherwise disabled)
    unsafe {
        *((PLIC + (UART0_IRQ * 4)) as *mut u32) = 1;
        *((PLIC + (VIRTIO0_IRQ * 4)) as *mut u32) = 1;
    }
}

pub fn init_hart() {
    unsafe {
        let _lock = Cpus::lock_mycpu();
        let hart = Cpus::get_id();

        // set enable bits for this hart's S-mode for uart and virtio disk
        *(PLIC_SENABLE(hart) as *mut u32) = (1 << UART0_IRQ) | (1 << VIRTIO0_IRQ);

        // set this hart's S-mode priority threshold to 0
        *(PLIC_SPRIORITY(hart) as *mut u32) = 0;
    }
}

// Ask PLIC what interrupt we should server.
pub fn claim() -> u32 {
    unsafe {
        let _lock = Cpus::lock_mycpu();
        let hart = Cpus::get_id();
        *(PLIC_SCLAIM(hart) as *mut u32)
    }
}

// Tell PLIC we've served this IRQ.
pub fn complete(irq: u32) {
    unsafe {
        let _lock = Cpus::lock_mycpu();
        let hart = Cpus::get_id();
        *(PLIC_SCLAIM(hart) as *mut u32) = irq;
    }
}
