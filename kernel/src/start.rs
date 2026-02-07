use core::arch::asm;

use crate::param::NCPU;
use crate::riscv::registers::*;

#[repr(C, align(16))]
struct Stack([u8; 4096 * NCPU]);

#[unsafe(no_mangle)]
static mut STACK0: Stack = Stack([0; 4096 * NCPU]);

unsafe extern "C" {
    fn main() -> !;
}

/// Ask each hart to generate timer interrupts.
unsafe fn timer_init() {
    unsafe {
        // enable supervisor-mode timer interrupts.
        mie::write(mie::read() | mie::STIE);

        // enable the sstc extension (i.e. stimecmp).
        menvcfg::write(menvcfg::read() | (1 << 63));

        // allow supervisor to use stimecmp and time.
        mcounteren::write(mcounteren::read() | 2);

        // ask for the very first timer interrupt.
        stimecmp::write(time::read() + 1000000);
    }
}

/// Entry point for each hart.
///
/// # Safety
/// This function is called from `entry.rs`.
pub unsafe fn start() -> ! {
    unsafe {
        // set previous privilege mode to supervisor
        // when `mret` is called at the end of this function,
        // this is the mode we will be going "back" to
        mstatus::set_mpp(mstatus::MPP_SUPERVISOR);

        // set the exception return instruction address to main
        // when `mret` is called at the end of this function,
        // this is the address we are going "back" to
        mepc::write(main as *const () as usize);

        // disable virtual address translation in supervisor mode
        satp::write(0);

        // delegate all interrupts and exceptions to supervisor mode
        medeleg::write(0xffff);
        mideleg::write(0xffff);
        sie::write(sie::read() | sie::SEIE | sie::STIE | sie::SSIE);

        // configure physical memory protection to give supervisor mode
        // access to all of physical memory
        pmpaddr0::write(0x3fffffffffffff);
        pmpcfg0::write(0xf);

        timer_init();

        let id = mhartid::read();
        tp::write(id);

        asm!("mret", options(noreturn));
    }
}
