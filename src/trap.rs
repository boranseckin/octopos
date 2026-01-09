use crate::kernelvec::kernelvec;
use crate::memlayout::{TRAMPOLINE, UART0_IRQ, VIRTIO0_IRQ};
use crate::plic;
use crate::println;
use crate::proc::{self, Cpus};
use crate::riscv::{
    PGSIZE, interrupts,
    registers::{satp, scause, sepc, sstatus, stimecmp, stval, stvec, time, tp},
};
use crate::spinlock::SpinLock;
use crate::uart::UART;

unsafe extern "C" {
    fn trampoline();
    fn uservec();
    fn userret();
}

static TICKS_LOCK: SpinLock<usize> = SpinLock::new(0, "time");

pub fn init() {
    // No work since lock is already initialized
}

// Set up to take exceptions and traps while in the kernel.
pub fn init_hart() {
    unsafe { stvec::write(kernelvec as *const () as usize) };
}

// Handles an interrupt, exception, or system call from user space.
// Called from trampoline.S
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usertrap() {
    unsafe {
        // make sure interrupt came from user space
        assert!(
            (sstatus::read() & sstatus::SPP) == 0,
            "usertrap: not from user mode"
        );

        // send subsequent interrupts and exceptions to kerneltrap, since we are in kernel mode now
        stvec::write(kernelvec as *const () as usize);

        let proc = Cpus::myproc().unwrap();
        let data = proc.data_mut();
        let trapframe = data.trapframe.as_mut().unwrap();

        // save user program counter in case, this handler yields to another core, and the new core
        // switches to user space, overwriting sepc.
        trapframe.epc = sepc::read();

        let scause = scause::Scause::from(scause::read());
        let mut which_dev = None;

        match scause.cause() {
            // System call
            scause::Trap::Exception(scause::Exception::EnvironmentCall) => {
                if proc.inner.lock().killed {
                    proc::exit(-1);
                }

                // sepc points to the ecall instruction, but we want to return to the next instruction.
                trapframe.epc += 4;

                // an interrupt will change sepc, scause, and sstatus, so enable only now that we're
                // done with those registers.
                interrupts::enable();

                todo!("syscall()")
            }

            // device interrupt
            scause::Trap::Interrupt(intr)
                if {
                    which_dev = dev_intr(intr);
                    which_dev.is_some()
                } =>
            {
                // dev_intr handles the interrupt if it is a device interrupt
                // nothing to do
            }

            // something else
            _ => {
                let mut inner = proc.inner.lock();

                println!(
                    "usertrap: unexpected scause=0x{:X} pid={:?} sepc=0x{:X} stval=0x{:X}",
                    scause.bits(),
                    inner.pid,
                    sepc::read(),
                    stval::read(),
                );

                inner.killed = true;
            }
        }

        if proc.inner.lock().killed {
            proc::exit(-1);
        }

        if Some(InterruptType::Timer) == which_dev {
            proc::r#yield();
        }

        usertrapret();
    }
}

// Return to user space.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usertrapret() {
    let proc = Cpus::myproc().unwrap();

    // we're about to switch the destination of traps from
    // kerneltrap() to usertrap(), so turn off interrupts until
    // we're back in user space, where usertrap() is correct.
    interrupts::disable();

    // send syscalls, interrupts, and exceptions to uservec in trampoline.S
    let trampoline_uservec =
        TRAMPOLINE + (uservec as *const () as usize - trampoline as *const () as usize);
    unsafe { stvec::write(trampoline_uservec) };

    // set up trapframe values that uservec will need when
    // the process next traps into the kernel.
    let data = unsafe { proc.data_mut() };
    let trapframe = data.trapframe.as_mut().unwrap();
    trapframe.kernel_satp = unsafe { satp::read() };
    trapframe.kernel_sp = data.kstack.0 + PGSIZE;
    trapframe.kernel_trap = usertrap as *const () as usize;
    trapframe.kernel_hartid = unsafe { tp::read() };

    // set up the registers that trampoline.S's sret will use to get to user space.

    // set Supervisor Previous Privilege mode to User.
    let mut x = unsafe { sstatus::read() };
    x &= !sstatus::SPP; // clear SPP to 0 for user mode
    x |= sstatus::SPIE; // enable interrupts in user mode
    unsafe { sstatus::write(x) };

    // set S Exception Program Counter to the saved user pc.
    unsafe { sepc::write(trapframe.epc) };

    // tell trampoline.S the user page table to switch to.
    let user_satp = satp::make(data.pagetable.as_ref().unwrap().0.as_pa().0);

    // jump to userret in trampoline.S at the top of memory, which
    // switches to the user page table, restores user registers,
    // and switches to user mode with sret.
    unsafe {
        let trampoline_userret: usize =
            TRAMPOLINE + (userret as *const () as usize - trampoline as *const () as usize);
        let trampoline_userret: extern "C" fn(usize) -> ! =
            core::mem::transmute(trampoline_userret);
        trampoline_userret(user_satp);
    }
}

// Interrupts and exceptions from the kernel code go here via kernelvec, on whatever the current
// kernel stack is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kerneltrap() {
    unsafe {
        let sepc = sepc::read();
        let sstatus = sstatus::read();
        let scause = scause::Scause::from(scause::read());

        assert!(
            sstatus & sstatus::SPP != 0,
            "kerneltrap: not from supervisor mode"
        );

        assert!(!interrupts::get(), "kerneltrap: interrupts enabled");

        let which_dev;

        // If we got exceptions in supervisor mode, or we got an interrupt from an unknown source,
        // it is fatal
        match scause.cause() {
            scause::Trap::Interrupt(intr)
                if {
                    which_dev = dev_intr(intr);
                    which_dev.is_some()
                } => {}

            _ => {
                println!(
                    "scause=0x{:X} sepc=0x{:X} stval=0x{:X}",
                    scause.bits(),
                    sepc::read(),
                    stval::read()
                );
                panic!("kerneltrap");
            }
        }

        // If we got a timer interrupt, give up the cpu for another process
        if Some(InterruptType::Timer) == which_dev && Cpus::myproc().is_some() {
            proc::r#yield();
        }

        // The yield() may have caused some traps to occur, so restore trap registers for use by
        // kernelvec.S's sepc instruction.
        sepc::write(sepc);
        sstatus::write(sstatus);
    }
}

pub fn clock_intr() {
    let _lock = Cpus::lock_mycpu();
    let hart = unsafe { Cpus::get_id() };

    if hart == 0 {
        let mut ticks = unsafe { TICKS_LOCK.lock() };
        *ticks += 1;
        proc::wakeup(&(*ticks) as *const _ as usize);
    }

    // Ask for the next timer interrupt.
    // This also clears the interrupt request.
    // 1_000_000 is about a tenth of a second.
    unsafe { stimecmp::write(time::read() + 1_000_000) };
}

#[derive(PartialEq, Eq)]
enum InterruptType {
    Device,
    Timer,
}

// Check if interrupt is from an external device or software timer.
fn dev_intr(intr: scause::Interrupt) -> Option<InterruptType> {
    match intr {
        // Supervisor external interrupt via PLIC
        scause::Interrupt::SupervisorExternal => {
            let irq = plic::claim();

            match irq as usize {
                UART0_IRQ => UART.handle_interrupt(),
                VIRTIO0_IRQ => todo!("virtio_disk_intr()"),
                _ => println!("unexpected interrupt irq = {}", irq),
            }

            if irq != 0 {
                plic::complete(irq);
            }

            Some(InterruptType::Device)
        }

        // Timer interrupt
        scause::Interrupt::SupervisorTimer => {
            clock_intr();
            Some(InterruptType::Timer)
        }

        // some other interrupt, we don't recognize
        _ => None,
    }
}
