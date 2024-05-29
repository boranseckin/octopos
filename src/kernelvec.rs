use core::arch::asm;

#[naked]
#[repr(align(16))]
#[no_mangle]
pub unsafe extern "C" fn timervec() -> ! {
    // start.rs has set up the memory that mscratch points to:
    // scratch[0,8,16] : register save area.
    // scratch[24] : address of CLINT's MTIMECMP register.
    // scratch[32] : desired interval between interrupts.

    asm!(
        "csrrw a0, mscratch, a0",
        "sd a1, 0(a0)",
        "sd a2, 8(a0)",
        "sd a3, 16(a0)",
        //
        "ld a1, 24(a0)",
        "ld a2, 32(a0)",
        "ld a3, 0(a1)",
        "add a3, a3, a2",
        "sd a3, 0(a1)",
        //
        "li a1, 2",
        "csrw sip, a1",
        //
        "ld a3, 16(a0)",
        "ld a2, 8(a0)",
        "ld a1, 0(a0)",
        "csrrw a0, mscratch, a0",
        "mret",
        options(noreturn)
    );
}
