pub mod registers {
    // Machine Hart (core) ID register, mhartid
    pub mod mhartid {
        use core::arch::asm;

        #[inline]
        pub unsafe fn read() -> usize {
            unsafe {
                let id: usize;
                asm!("csrr {}, mhartid", out(reg) id);
                id
            }
        }
    }

    // Machine Status register, mstatus
    pub mod mstatus {
        use core::arch::asm;

        pub const MPP_MASK: usize = 3 << 11;
        pub const MIE: usize = 1 << 3; // Machine Mode Interrupt enable

        // Machine Previous Privilege Mode
        pub enum MPP {
            Machine = 3,
            Supervisor = 1,
            User = 0,
        }

        #[inline]
        pub unsafe fn read() -> usize {
            unsafe {
                let bits: usize;
                asm!("csrr {}, mstatus", out(reg) bits);
                bits
            }
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw mstatus, {}", in(reg) bits);
            }
        }

        #[inline]
        pub fn set_mpp(mpp: MPP) {
            unsafe {
                let mut value = read();
                value &= !MPP_MASK;
                value |= (mpp as usize) << 11;
                write(value);
            }
        }
    }

    // Supervisor Status register, sstatus
    pub mod sstatus {
        use core::arch::asm;

        pub const SPP: usize = 1 << 8; // Previous mode, 1=Supervisor, 0=User
        pub const SPIE: usize = 1 << 5; // Supervisor Previous Interrupt Enable
        pub const UPIE: usize = 1 << 4; // User Previous Interrupt Enable
        pub const SIE: usize = 1 << 1; // Supervisor Interrupt Enable
        pub const UIE: usize = 1 << 0; // User Interrupt Enable

        #[inline]
        pub unsafe fn read() -> usize {
            unsafe {
                let bits: usize;
                asm!("csrr {}, sstatus", out(reg) bits);
                bits
            }
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw sstatus, {}", in(reg) bits);
            }
        }
    }

    // Supervisor Trap Cause
    pub mod scause {
        use core::arch::asm;

        #[inline]
        pub unsafe fn read() -> usize {
            unsafe {
                let bits: usize;
                asm!("csrr {}, scause", out(reg) bits);
                bits
            }
        }
    }

    // Supervisor Trap-Vector Base Address
    pub mod stvec {
        use core::arch::asm;

        #[inline]
        pub unsafe fn read() -> usize {
            unsafe {
                let bits: usize;
                asm!("csrr {}, stvec", out(reg) bits);
                bits
            }
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw stvec, {}", in(reg) bits);
            }
        }
    }

    // Supervisor Exception Program Counter
    // holds the instruction address to which a return from exception will go
    pub mod sepc {
        use core::arch::asm;

        #[inline]
        pub unsafe fn read() -> usize {
            unsafe {
                let bits: usize;
                asm!("csrr {}, sepc", out(reg) bits);
                bits
            }
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw sepc, {}", in(reg) bits);
            }
        }
    }

    // Machien Exception Program Counter register, mepc
    pub mod mepc {
        use core::arch::asm;

        #[inline]
        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw mepc, {}", in(reg) bits);
            }
        }
    }

    // Machine Exception Delegation register, medeleg
    pub mod medeleg {
        use core::arch::asm;

        #[inline]
        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw medeleg, {}", in(reg) bits);
            }
        }
    }

    // Machine Interrupt Delegation register, medeleg
    pub mod mideleg {
        use core::arch::asm;

        #[inline]
        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw mideleg, {}", in(reg) bits);
            }
        }
    }

    // Machine Scratch register, mscratch
    pub mod mscratch {
        use core::arch::asm;

        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw mscratch, {}", in(reg) bits);
            }
        }
    }

    // Machine Trap Vector Register, mtvec
    pub mod mtvec {
        use core::arch::asm;

        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw mtvec, {}", in(reg) bits);
            }
        }
    }

    // Physical Memory Protection Config register, pmpcfg0
    pub mod pmpcfg0 {
        use core::arch::asm;

        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw pmpcfg0, {}", in(reg) bits);
            }
        }
    }

    // Physical Memory Protection Address register, pmpaddr0
    pub mod pmpaddr0 {
        use core::arch::asm;

        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw pmpaddr0, {}", in(reg) bits);
            }
        }
    }

    // Supervisor Interrupt Enable register, sie
    pub mod sie {
        use core::arch::asm;

        pub const SEIE: usize = 1 << 9; // external
        pub const STIE: usize = 1 << 5; // timer
        pub const SSIE: usize = 1 << 1; // software

        #[inline]
        pub unsafe fn read() -> usize {
            unsafe {
                let bits: usize;
                asm!("csrr {}, sie", out(reg) bits);
                bits
            }
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw sie, {}", in(reg) bits);
            }
        }
    }

    // Machine Interrupt Enable register, mie
    pub mod mie {
        use core::arch::asm;

        pub const MEIE: usize = 1 << 11; // external
        pub const MTIE: usize = 1 << 7; // timer
        pub const MSIE: usize = 1 << 3; // software

        #[inline]
        pub unsafe fn read() -> usize {
            unsafe {
                let bits: usize;
                asm!("csrr {}, mie", out(reg) bits);
                bits
            }
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw mie, {}", in(reg) bits);
            }
        }
    }

    // Supervisor Address Translation and Protection register, satp
    // holds the address of the page table
    pub mod satp {
        use core::arch::asm;

        // use riscv's sv39 page table scheme
        const SV39: usize = 8 << 60;

        pub const fn make(pagetable: usize) -> usize {
            SV39 | (pagetable >> 12)
        }

        #[inline]
        pub unsafe fn read() -> usize {
            unsafe {
                let bits: usize;
                asm!("csrr {}, satp", out(reg) bits);
                bits
            }
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("csrw satp, {}", in(reg) bits);
            }
        }
    }

    // Thread Pointer register, tp
    pub mod tp {
        use core::arch::asm;

        #[inline]
        pub unsafe fn read() -> usize {
            unsafe {
                let bits: usize;
                asm!("mv {}, tp", out(reg) bits);
                bits
            }
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            unsafe {
                asm!("mv tp, {}", in(reg) bits);
            }
        }
    }

    pub mod vma {
        use core::arch::asm;

        #[inline]
        // Synchronizes updates to the supervisor memory-management data structers.
        // When used with r1=0 and r2=0, The fence also invalidates all address-translation cache entries, for all address spaces.
        pub unsafe fn sfence() {
            unsafe {
                asm!("sfence.vma zero, zero");
            }
        }
    }
}

pub mod interrupts {
    use super::registers::sstatus;

    #[inline]
    pub fn enable() {
        unsafe { sstatus::write(sstatus::read() | sstatus::SIE) };
    }

    #[inline]
    pub fn disable() {
        unsafe { sstatus::write(sstatus::read() | !sstatus::SIE) };
    }

    #[inline]
    pub fn get() -> bool {
        unsafe { (sstatus::read() & sstatus::SIE) != 0 }
    }
}

// number of bits to offset within a page
pub const PGSHIFT: usize = 12;
// number of bytes per page
pub const PGSIZE: usize = 1 << PGSHIFT;

pub const fn pg_round_up(size: usize) -> usize {
    (size + PGSIZE - 1) & !(PGSIZE - 1)
}

pub const fn pg_round_down(addr: usize) -> usize {
    addr & !(PGSIZE - 1)
}

pub const PTE_V: usize = 1 << 0;
pub const PTE_R: usize = 1 << 1;
pub const PTE_W: usize = 1 << 2;
pub const PTE_X: usize = 1 << 3;
pub const PTE_U: usize = 1 << 4;

pub const fn pa_to_pte(pa: usize) -> usize {
    (pa >> 12) << 10
}

pub const fn pte_to_pa(pte: usize) -> usize {
    (pte >> 10) << 12
}

pub const fn pte_flags(pte: usize) -> usize {
    pte & 0x3FF
}

// extraxt the three 9-bit page table indices from a virtual address
pub const PXMASK: usize = 0x1FF; // 9 bits

// returns the amount to shift-left to get to the correct page table index
pub const fn px_shift(level: usize) -> usize {
    // 12-bit page offset + 9-bit per level
    PGSHIFT + (9 * level)
}

// returns the page table index of the va for the corresponding level
pub const fn px(level: usize, va: usize) -> usize {
    (va >> px_shift(level)) & PXMASK
}

// one beyond the highest possible virtual address
// (3 x 9-bit pages) + 12-bit offset
//
// this is 1-bit less than the max allowed by Sv39 to avoid having to sign-extend virtual addresses
// that have the high bit set
pub const MAXVA: usize = 1 << (9 + 9 + 9 + 12 - 1);
