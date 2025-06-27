use core::arch::naked_asm;

use crate::memlayout::TRAPFRAME;

// Low-level code to handle traps from use space into the kernel, and return from kernel to user.
//
// The kernel maps the page holding this code at the same virtual address (TRAMPOLINE) in user and
// kernel space so that it continues to work when it switches page tables.
//
// kernel.ld causes this code to start at a page boundary.
//
// uservec and userret are naked functions since they only run asm

#[unsafe(no_mangle)]
#[unsafe(link_section = "trampsec")]
#[align(4)]
pub unsafe extern "C" fn trampoline() {
    unreachable!();

    #[unsafe(no_mangle)]
    #[unsafe(naked)]
    #[unsafe(link_section = "trampsec")]
    unsafe extern "C" fn uservec() -> ! {
        unsafe {
            // trap.rs sets stvec to point here, so traps from user space start here in supervisor
            // mode but with a user page table. To continue handling the interrupt in supervisor
            // mode, this code switches the page table from user's to kernel's.
            // Inside the trap, there is only 1 scratch register to use (sscratch). We use this
            // register to hold the original value of a0, and use a0 as the general purpose
            // register to hold the address of the TRAPFRAME.
            naked_asm!(
                // save user a0 in sscratch so a0 can be used to get at TRAPFRAME
                "csrw sscratch, a0",

                // each process has a separate p->trapframe memory area, but it's mapped to the same
                // virtual address (TRAPFRAME) in every process' user page table.
                "li a0, {tp}",

                // save the user registers in TRAPFRAME
                "sd ra, 40(a0)",
                "sd sp, 48(a0)",
                "sd gp, 56(a0)",
                "sd tp, 64(a0)",
                "sd t0, 72(a0)",
                "sd t1, 80(a0)",
                "sd t2, 88(a0)",
                "sd s0, 96(a0)",
                "sd s1, 104(a0)",
                "sd a1, 120(a0)",
                "sd a2, 128(a0)",
                "sd a3, 136(a0)",
                "sd a4, 144(a0)",
                "sd a5, 152(a0)",
                "sd a6, 160(a0)",
                "sd a7, 168(a0)",
                "sd s2, 176(a0)",
                "sd s3, 184(a0)",
                "sd s4, 192(a0)",
                "sd s5, 200(a0)",
                "sd s6, 208(a0)",
                "sd s7, 216(a0)",
                "sd s8, 224(a0)",
                "sd s9, 232(a0)",
                "sd s10, 240(a0)",
                "sd s11, 248(a0)",
                "sd t3, 256(a0)",
                "sd t4, 264(a0)",
                "sd t5, 272(a0)",
                "sd t6, 280(a0)",

                // save the user a0 in p->trapframe->a0
                "csrr t0, sscratch",
                "sd t0, 112(a0)",

                // initailize kernel stack pointer from p->trapframe->kernel_sp
                "ld sp, 8(a0)",

                // make tp hold the current hartid from p->trapframe->kernel_hartid
                "ld tp, 32(a0)",

                // load the address of usertrap() from p->trapframe->kernel_trap
                "ld t0, 16(a0)",

                // fetch the kernel page table address from p->trapframe->kernel_satp
                "ld t1, 0(a0)",

                // wait for any previous memory operations to complete,
                // so that they suse the user page table
                "sfence.vma zero, zero",

                // install the kernel page table
                "csrw satp, t1",

                // flush now-tale user entries from the TLB
                "sfence.vma zero, zero",

                // jump to usertrap(), which does not return
                "jr t0",

                tp = const TRAPFRAME,
            );
        }
    }

    #[unsafe(no_mangle)]
    #[unsafe(naked)]
    #[unsafe(link_section = "trampsec")]
    unsafe extern "C" fn userret(page_table: usize) -> ! {
        unsafe {
            // called by usertrapret() in trap.rs to switch from kernel to user
            // a0 includes the user page table base, passed as the only argument
            naked_asm!(
                // switch to user page table
                "sfence.vma zero, zero",
                "csrw satp, a0",
                "sfence.vma zero, zero",

                "li a0, {tf}",

                // restore all but a0 from TRAPFRAME
                "ld ra, 40(a0)",
                "ld sp, 48(a0)",
                "ld gp, 56(a0)",
                "ld tp, 64(a0)",
                "ld t0, 72(a0)",
                "ld t1, 80(a0)",
                "ld t2, 88(a0)",
                "ld s0, 96(a0)",
                "ld s1, 104(a0)",
                "ld a1, 120(a0)",
                "ld a2, 128(a0)",
                "ld a3, 136(a0)",
                "ld a4, 144(a0)",
                "ld a5, 152(a0)",
                "ld a6, 160(a0)",
                "ld a7, 168(a0)",
                "ld s2, 176(a0)",
                "ld s3, 184(a0)",
                "ld s4, 192(a0)",
                "ld s5, 200(a0)",
                "ld s6, 208(a0)",
                "ld s7, 216(a0)",
                "ld s8, 224(a0)",
                "ld s9, 232(a0)",
                "ld s10, 240(a0)",
                "ld s11, 248(a0)",
                "ld t3, 256(a0)",
                "ld t4, 264(a0)",
                "ld t5, 272(a0)",
                "ld t6, 280(a0)",

                // restore user a0
                "ld a0, 112(a0)",

                // return to user mode and user pc
                // usertrapret() set up sstatus and sepc
                "sret",

                tf = const TRAPFRAME,
            );
        }
    }
}
