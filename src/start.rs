use core::arch::asm;

use crate::kernelvec::timervec;
use crate::memlayout::{CLINT_MTIME, CLINT_MTIMECMP};
use crate::param::NCPU;
use crate::riscv::registers::*;

#[repr(C, align(16))]
struct Stack([u8; 4096 * NCPU]);

#[unsafe(no_mangle)]
static mut STACK0: Stack = Stack([0; 4096 * NCPU]);

unsafe extern "C" {
    fn main() -> !;
}

pub unsafe fn start() -> ! {
    unsafe {
        // set previous privilege mode to supervisor
        // when `mret` is called at the end of this function,
        // this is the mode we will be going "back" to
        mstatus::set_mpp(mstatus::MPP::Supervisor);

        // set the exception return instruction address to main
        // when `mret` is called at the end of this function,
        // this is the address we are going "back" to
        mepc::write(main as usize);

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

static mut TIMER_SCRATCH: [[u64; 5]; NCPU] = [[0; 5]; NCPU];

unsafe fn timer_init() {
    unsafe {
        let id = mhartid::read();

        // ask CLINT for a timer interrupt
        let interval = 1_000_000; // cycles; about 1/10th second in qemu
        let mtimecmp = CLINT_MTIMECMP(id) as *mut u64;
        let mtime = CLINT_MTIME as *const u64;
        mtimecmp.write_volatile(mtime.read_volatile() + interval);

        // prepare information in scratch[] for timervec
        // scratch[0..2] : space for timervec to save registers
        // scratch[3]    : address of CLINT MTIMECMP register
        // scratch[4]    : desired interval (in cycles) between timer interrupts
        let scratch = &mut TIMER_SCRATCH[id];
        scratch[3] = mtimecmp as u64;
        scratch[4] = interval;
        mscratch::write(scratch.as_mut_ptr() as usize);

        // set machine mode trap handler
        mtvec::write(timervec as usize);

        // enable machine mode interrupts
        mstatus::write(mstatus::read() | mstatus::MIE);

        // enable machine mode timer interrupts
        mie::write(mie::read() | mie::MTIE);
    }
}
