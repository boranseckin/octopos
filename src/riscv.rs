pub mod registers {
    // Machine Hart (core) ID register, mhartid
    pub mod mhartid {
        use core::arch::asm;

        #[inline]
        pub unsafe fn read() -> usize {
            let id: usize;
            asm!("csrr {}, mhartid", out(reg) id);
            id
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
            let bits: usize;
            asm!("csrr {}, mstatus", out(reg) bits);
            bits
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            asm!("csrw mstatus, {}", in(reg) bits);
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
            let bits: usize;
            asm!("csrr {}, sstatus", out(reg) bits);
            bits
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            asm!("csrw sstatus, {}", in(reg) bits);
        }
    }

    // Machien Exception Program Counter register, mepc
    pub mod mepc {
        use core::arch::asm;

        #[inline]
        pub unsafe fn write(x: usize) {
            asm!("csrw mepc, {}", in(reg) x);
        }
    }

    // Machine Exception Delegation register, medeleg
    pub mod medeleg {
        use core::arch::asm;

        #[inline]
        pub unsafe fn write(bits: usize) {
            asm!("csrw medeleg, {}", in(reg) bits);
        }
    }

    // Machine Interrupt Delegation register, medeleg
    pub mod mideleg {
        use core::arch::asm;

        #[inline]
        pub unsafe fn write(bits: usize) {
            asm!("csrw mideleg, {}", in(reg) bits);
        }
    }

    // Machine Scratch register, mscratch
    pub mod mscratch {
        use core::arch::asm;

        pub unsafe fn write(bits: usize) {
            asm!("csrw mscratch, {}", in(reg) bits);
        }
    }

    // Machine Trap Vector Register, mtvec
    pub mod mtvec {
        use core::arch::asm;

        pub unsafe fn write(bits: usize) {
            asm!("csrw mtvec, {}", in(reg) bits);
        }
    }

    // Physical Memory Protection Config register, pmpcfg0
    pub mod pmpcfg0 {
        use core::arch::asm;

        pub unsafe fn write(bits: usize) {
            asm!("csrw pmpcfg0, {}", in(reg) bits);
        }
    }

    // Physical Memory Protection Address register, pmpaddr0
    pub mod pmpaddr0 {
        use core::arch::asm;

        pub unsafe fn write(bits: usize) {
            asm!("csrw pmpaddr0, {}", in(reg) bits);
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
            let bits: usize;
            asm!("csrr {}, sie", out(reg) bits);
            bits
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            asm!("csrw sie, {}", in(reg) bits);
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
            let bits: usize;
            asm!("csrr {}, mie", out(reg) bits);
            bits
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            asm!("csrw mie, {}", in(reg) bits);
        }
    }

    // Supervisor Address Translation and Protection register, satp
    pub mod satp {
        use core::arch::asm;

        #[inline]
        pub unsafe fn read() -> usize {
            let bits: usize;
            asm!("csrr {}, satp", out(reg) bits);
            bits
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            asm!("csrw satp, {}", in(reg) bits);
        }
    }

    // Thread Pointer register, tp
    pub mod tp {
        use core::arch::asm;

        #[inline]
        pub unsafe fn read() -> usize {
            let bits: usize;
            asm!("mv {}, tp", out(reg) bits);
            bits
        }

        #[inline]
        pub unsafe fn write(bits: usize) {
            asm!("mv tp, {}", in(reg) bits);
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

// one beyond the highest possible virtual address
// (3 x 9-bit pages) + 12-bit offset
//
// this is 1-bit less than the max allowed by Sv39 to avoid having to sign-extend virtual addresses
// that have the high bit set
pub const MAXVA: usize = 1 << (9 + 9 + 9 + 12 - 1);
