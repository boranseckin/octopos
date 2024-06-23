use crate::println;
use crate::riscv::{
    interrupts,
    registers::{scause, sepc, sstatus},
};

extern "C" {
    fn trampoline();
    fn uservec();
    fn userret();
}

// TODO: remove address prints when these functions are actually used.
// Currently the linker is optimizing the trampsec out, this is a hack.
pub fn init() {
    unsafe {
        println!("tramp: {:#X}", trampoline as usize);
        println!("vec: {:#X}", uservec as usize);
        println!("ret: {:#X}", userret as usize);
        println!();
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
