use core::arch::naked_asm;

use crate::param::BOOTSTACK_SIZE;
use crate::start::{STACK0, start};

/// Entry point for the kernel.
///
/// # Safety
/// This function is called from the assembly startup code.
#[unsafe(naked)]
#[unsafe(link_section = ".entry")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _entry() -> ! {
    // In RISC-V, the stack grows downwards. Stack pointer of each CPU points
    // to the top of the stack. Each stack is BOOTSTACK_SIZE bytes.
    naked_asm!(
        "la sp, {stack}",    // load address of STACK0 to stack pointer
        "li a0, {size}",    // load immediate value to a0
        "csrr a1, mhartid", // read hartid from control and status register to a1
        "addi a1, a1, 1",   // add immediate 1 to a1
        "mul a0, a0, a1",   // multiply a0 and a1 into a0
        "add sp, sp, a0",   // add a0 to sp
        "call {start}",     // call start
        stack = sym STACK0,
        size = const BOOTSTACK_SIZE,
        start = sym start,
    )
}
