// Physical memory layout

// qemu -machine virt is set up like this,
// based on qemu's hw/riscv/virt.c:
//
// 00001000 -- boot ROM, provided by qemu
// 02000000 -- CLINT
// 0C000000 -- PLIC
// 10000000 -- uart0
// 10001000 -- virtio disk
// 80000000 -- boot ROM jumps here in machine mode
//             -kernel loads the kernel here
// unused RAM after 80000000.

// the kernel uses physical memory thus:
// 80000000 -- entry.S, then kernel text and data
// end -- start of kernel page allocation area
// PHYSTOP -- end RAM used by the kernel

pub const UART0: usize = 0x10000000;

// Core Local Interrupter (CLINT)
pub const CLINT: usize = 0x2000000;
pub const fn clint_mtimecmp(hartid: usize) -> usize {
    CLINT + 0x4000 + 8 * hartid
}
pub const CLINT_MTIME: usize = CLINT + 0xBFF8; // Cycles since boot
