use crate::kernelvec::kernelvec;
use crate::println;
use crate::riscv::{
    interrupts,
    registers::{scause, sepc, sstatus, stvec},
};
use crate::spinlock::Mutex;

extern "C" {
    fn trampoline();
    fn uservec();
    fn userret();
}

static mut TICKS_LOCK: Mutex<i32> = Mutex::new(0, "time");

pub fn init() {
    // No work since lock is already initialized
}

// set up to take exceptions and traps while in the kernel
pub fn init_hart() {
    unsafe {
        stvec::write(kernelvec as usize);
    }
}

// interrupts and exceptions from the kernel code go here via kernelvec
// on whatever the current kernel stack is
#[no_mangle]
pub unsafe extern "C" fn kerneltrap() {
    // let which_dev;

    let sepc = sepc::read();
    let sstatus = sstatus::read();
    let scause = scause::read();

    assert!(
        sstatus & sstatus::SPP == 0,
        "kerneltrap: not from supervisor mode"
    );

    assert!(!interrupts::get(), "kerneltrap: interrupts enabled");

    todo!()
}
