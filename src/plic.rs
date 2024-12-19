use crate::{
    memlayout::{PLIC, UART0_IRQ, VIRTIO0_IRQ, plic_senable, plic_spriority},
    proc::{CPUS, Cpu, Cpus},
};

// RISCV Platfform Level Interrupt Controller (PLIC)

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
        *(plic_senable(hart) as *mut u32) = (1 << UART0_IRQ) | (1 << VIRTIO0_IRQ);

        // set this hart's S-mode priority threshold to 0
        *(plic_spriority(hart) as *mut u32) = 0;
    }
}
