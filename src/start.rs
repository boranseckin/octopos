use crate::kernelvec::timervec;
use crate::memlayout::{clint_mtimecmp, CLINT_MTIME};
use crate::param::NCPU;
use crate::riscv::registers::*;

use core::arch::asm;
use core::hint::unreachable_unchecked;

#[repr(C, align(16))]
struct Stack([u8; 4096 * NCPU]);

#[no_mangle]
static mut STACK0: Stack = Stack([0; 4096 * NCPU]);

extern "C" {
    fn main() -> !;
}

pub unsafe fn start() -> ! {
    // Set M Previous Privilege mode to Supervisor
    mstatus::set_mpp(mstatus::MPP::Supervisor);

    // set the exception return instruction address to main
    mepc::write(main as usize);

    // disable paging
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

    asm!("mret");

    unreachable_unchecked();
}

static mut TIMER_SCRATCH: [[u64; 5]; NCPU] = [[0; 5]; NCPU];

unsafe fn timer_init() {
    let id = mhartid::read();

    let interval = 1_000_000; // cycles; about 1/10th second in qemu
    let mtimecmp = clint_mtimecmp(id) as *mut u64;
    let mtime = CLINT_MTIME as *const u64;
    mtimecmp.write_volatile(mtime.read_volatile() + interval);

    let scratch = &mut TIMER_SCRATCH[id];
    scratch[3] = mtimecmp as u64;
    scratch[4] = interval;
    mscratch::write(scratch.as_mut_ptr() as usize);

    mtvec::write(timervec as usize);

    mstatus::write(mstatus::read() | mstatus::MIE);

    mie::write(mie::read() | mie::MTIE);
}
