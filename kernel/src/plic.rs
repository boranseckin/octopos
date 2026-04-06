// RISCV Platfform Level Interrupt Controller (PLIC)
// https://github.com/riscv/riscv-plic-spec/blob/master/riscv-plic.adoc

use crate::memlayout::{
    E1000_IRQ, PLIC, PLIC_SCLAIM, PLIC_SENABLE, PLIC_SPRIORITY, UART0_IRQ, VIRTIO0_IRQ,
};
use crate::proc;

/// Asks PLIC what interrupt we should server.
pub fn claim() -> u32 {
    let _lock = proc::lock_current_cpu();

    // # Safety: cpu is locked
    unsafe {
        let hart = proc::current_id();
        *(PLIC_SCLAIM(hart) as *mut u32)
    }
}

/// Informs PLIC that we have served this IRQ.
pub fn complete(irq: u32) {
    let _lock = proc::lock_current_cpu();

    // # Safety: cpu is locked
    unsafe {
        let hart = proc::current_id();
        *(PLIC_SCLAIM(hart) as *mut u32) = irq;
    }
}

/// Initializes the PLIC.
///
/// # Safety
/// This function must be called only once during system initialization.
pub unsafe fn init() {
    // set desired IRQ priorities non-zero (otherwise disabled)
    unsafe {
        *((PLIC + (UART0_IRQ * 4)) as *mut u32) = 1;
        *((PLIC + (VIRTIO0_IRQ * 4)) as *mut u32) = 1;
        *((PLIC + (E1000_IRQ * 4)) as *mut u32) = 1;
    }

    println!("plic init");
}

/// Initializes PLIC for this hart.
///
/// # Safety
/// This function must be called only once per hart during system initialization.
pub unsafe fn init_hart() {
    unsafe {
        let _lock = proc::lock_current_cpu();
        // # Safety: cpu is locked
        let hart = proc::current_id();

        // set enable bits for this hart's S-mode for uart and virtio disk
        // word 0 (IRQs 0-31): UART0 (10) and VIRTIO0 (1)
        *(PLIC_SENABLE(hart) as *mut u32) = (1 << UART0_IRQ) | (1 << VIRTIO0_IRQ);
        // word 1 (IRQs 32-63): E1000 (33 = bit 1 of word 1)
        *(PLIC_SENABLE(hart) as *mut u32).add(1) = 1 << (E1000_IRQ - 32);

        // set this hart's S-mode priority threshold to 0
        *(PLIC_SPRIORITY(hart) as *mut u32) = 0;
    }
}
