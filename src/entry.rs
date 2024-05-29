use crate::start::start;
use core::arch::asm;

#[link_section = ".entry"]
#[no_mangle]
pub unsafe extern "C" fn _entry() -> ! {
    asm!(
        "la sp, STACK0",    // load address of STACK0 to stack pointer
        "li a0, 4096",      // load immediate value to a0
        "csrr a1, mhartid", // read hartid from control and status register to a1
        "addi a1, a1, 1",   // add immediate 1 to a1
        "mul a0, a0, a1",   // multiply a0 and a1 into a0
        "add sp, sp, a0",   // add a0 to sp
    );

    start()
}
