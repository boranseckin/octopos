use crate::kernelvec::kernelvec;
use crate::memlayout::{UART0_IRQ, VIRTIO0_IRQ};
use crate::proc::Cpus;
use crate::riscv::registers::{stimecmp, stval, time};
use crate::riscv::{
    interrupts,
    registers::{scause, sepc, sstatus, stvec},
};
use crate::spinlock::Mutex;
use crate::uart::UART;
use crate::{plic, println};

unsafe extern "C" {
    fn trampoline();
    fn uservec();
    fn userret();
}

static TICKS_LOCK: Mutex<i32> = Mutex::new(0, "time");

pub fn init() {
    // No work since lock is already initialized
}

// set up to take exceptions and traps while in the kernel
pub fn init_hart() {
    unsafe {
        stvec::write(kernelvec as usize);
    }
}

// handles an interrupt, exception, or system call from user space.
// called from trampoline.S
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usertrap() {
    unsafe {
        // make sure interrupt came from user space
        assert!(
            (sstatus::read() & sstatus::SPP) == 0,
            "usertrap: not from user mode"
        );

        // send subsequent interrupts and exceptions to kerneltrap, since we are in kernel mode now
        stvec::write(kernelvec as usize);

        let proc = Cpus::myproc().unwrap();
        let data = proc.data_mut();
        let trapframe = data.trapframe.as_mut().unwrap();

        // save user program counter in case, this handler yields to another core, and the new core
        // switches to user space, overwriting sepc.
        trapframe.epc = sepc::read();

        let mut which_dev = None;

        // system call
        if scause::read() == 8 {
            if proc.inner.lock().killed {
                // TODO: proc::exit
            }

            // sepc points to the ecall instruction, but we want to return to the next instruction.
            trapframe.epc += 4;

            // an interrupt will change sepc, scause, and sstatus, so enable only now that we're
            // done with those registers.
            interrupts::enable();

            // TODO: syscall()
        } else if {
            which_dev = Some(dev_intr());
            which_dev == Some(InterruptType::External)
        } {
            // ok
        } else {
            let mut inner = proc.inner.lock();

            println!(
                "usertrap: unexpected scause: 0x{:X} pid={:?} sepc=0x{:X} stval=0x{:X}",
                scause::read(),
                inner.pid,
                sepc::read(),
                stval::read(),
            );

            inner.killed = true;
        }

        if proc.inner.lock().killed {
            // TODO: proc::exit
        }

        if which_dev == Some(InterruptType::External) {
            // TODO: yield
        }

        usertrapret();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn usertrapret() {}

// interrupts and exceptions from the kernel code go here via kernelvec
// on whatever the current kernel stack is
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kerneltrap() {
    unsafe {
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
}

pub fn clock_intr() {
    let _lock = Cpus::lock_mycpu();
    let hart = unsafe { Cpus::get_id() };

    if hart == 0 {
        let mut ticks = unsafe { TICKS_LOCK.lock() };
        *ticks += 1;
        todo!("wakeup");
    }

    // Ask fro the next timer interrupt.
    // This also clears the interrupt request.
    // 1000000 is about a tenth of a second.
    unsafe { stimecmp::write(time::read() + 1000000) };
}

#[derive(PartialEq, Eq)]
pub enum InterruptType {
    Internal,
    External,
    Other,
}

pub fn dev_intr() -> InterruptType {
    let scause = unsafe { scause::read() };

    match scause {
        // Supervisor external interrupt via PLIC
        0x8000_0000_0000_0009 => {
            let irq = plic::claim();

            match irq as usize {
                UART0_IRQ => UART.handle_interrupt(),
                VIRTIO0_IRQ => todo!(),
                _ => println!("unexpected interrupt irq = {irq}"),
            }

            if irq != 0 {
                plic::complete(irq);
            }

            InterruptType::External
        }
        // Timer interrupt
        0x8000_0000_0000_0005 => {
            clock_intr();
            InterruptType::Internal
        }
        // some other interrupt, we don't recognize
        _ => InterruptType::Other,
    }
}
