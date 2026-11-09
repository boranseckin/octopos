//! The kernel's two kinds of stack, and overflow checks for both.
//!
//! - `KernelStack`: one per process slot, `NKSTACK_PAGES` pages below the trampoline with an
//!   unmapped guard page underneath. A process runs on it during syscalls and traps.
//! - `BootStack`: one `BOOTSTACK_SIZE` slice of `STACK0` per hart, identity-mapped, with no guard
//!   page. `_entry` starts each hart on it, and the hart's scheduler keeps running on it forever.
//!
//! Both types only hold an address and never own memory. `ProcTable::map_stacks` maps the kstack
//! pages at boot and `STACK0` is a static, so neither is ever freed or unmapped.
//!
//! Overflow checks work by painting: a stack grows down from its top, and every frame overwrites
//! what was below `sp`. So a range filled with `PAINT_PATTERN` before use shows afterwards how deep
//! the stack went.
//! - Tripwire (always on): only the bottom `STACK_MARGIN` bytes are painted, and the scheduler
//!   checks them while nothing is running on that stack. A stack that came within the margin of
//!   overflowing panics with a message, instead of hanging on its guard page later (kstacks) or
//!   silently corrupting the next hart's slice (boot stacks).
//! - Measurement (`TRACK_STACK_USAGE`, kstacks only): the whole stack is painted, and
//!   `high_water` reports how much of it a process used by the time it exits.

use crate::memlayout;
use crate::param::{BOOTSTACK_SIZE, MAX_KERNEL_STACK_BUFFER, NKSTACK_PAGES, TRACK_STACK_USAGE};
use crate::riscv::PGSIZE;
use crate::start::STACK0;
use crate::vm::VA;

/// Written into unused stack as a marker.
/// Spells out `STACK_PA` in ASCII.
const PAINT_PATTERN: u64 = 0x5354_4143_4B5F_5041;

/// Bytes at the bottom of each stack that must stay untouched.
const STACK_MARGIN: usize = MAX_KERNEL_STACK_BUFFER;

/// # Safety
/// `bottom..bottom + len` must be mapped, 8-byte aligned, and not in use by any running code.
unsafe fn fill(bottom: VA, len: usize) {
    debug_assert_eq!(len % size_of::<u64>(), 0, "paint length is whole words");

    let base: *mut u64 = bottom.as_mut_ptr();
    let words = len / size_of::<u64>();

    // SAFETY: the range is mapped, aligned, and unused.
    (0..words).for_each(|i| unsafe {
        base.add(i).write_volatile(PAINT_PATTERN);
    });
}

/// # Safety
/// `bottom..bottom + STACK_MARGIN` must be mapped and 8-byte aligned, and must have been painted
/// with `fill()`. The stack must not be running while it is checked.
unsafe fn margin_intact(bottom: VA) -> bool {
    let base: *mut u64 = bottom.as_mut_ptr();
    let words = STACK_MARGIN / size_of::<u64>();

    // SAFETY: the range is mapped, aligned, and unused.
    (0..words).all(|i| unsafe { base.add(i).read_volatile() } == PAINT_PATTERN)
}

/// One process slot's kernel stack.
/// `NKSTACK_PAGES` pages below the trampoline, with an unmapped guard page underneath.
/// `ProcTable::map_stacks` maps every slot at boot, and they are never unmapped, so a `KernelStack`
/// always represents mapped, page-aligned memory.
#[derive(Debug, Clone, Copy)]
pub struct KernelStack {
    bottom: VA,
}

impl KernelStack {
    pub const SIZE: usize = NKSTACK_PAGES * PGSIZE;

    /// How much of each kstack is painted: the whole stack when measuring, otherwise only the
    /// tripwire margin. Kernel stacks only; boot stacks always paint `STACK_MARGIN`.
    const PAINT_SIZE: usize = if TRACK_STACK_USAGE {
        Self::SIZE
    } else {
        STACK_MARGIN
    };

    pub const fn for_slot(slot: usize) -> Self {
        let va = VA::new(memlayout::kstack(slot));
        Self { bottom: va }
    }

    pub fn bottom(&self) -> VA {
        self.bottom
    }

    pub fn top(&self) -> VA {
        self.bottom + Self::SIZE
    }

    /// Fills the bottom of the stack with a known pattern.
    /// If `TRACK_STACK_USAGE` is set, fills the whole stack.
    ///
    /// # Safety
    /// Nothing should be running on this stack.
    pub unsafe fn paint(&self) {
        unsafe { fill(self.bottom, Self::PAINT_SIZE) };
    }

    /// Returns whether the bottom `STACK_MARGIN` bytes of the stack still hold the pattern painted
    /// by `paint()`.
    ///
    /// # Safety
    /// Nothing should be running on this stack.
    pub unsafe fn check_margin(&self) -> bool {
        unsafe { margin_intact(self.bottom) }
    }

    /// Returns how many bytes of the painted byte range were used, measured from the top.
    /// Returns None if `TRACK_STACK_USAGE` is unset.
    ///
    /// Scans up from the bottom to the first overwritten word. Everything above it counts as used,
    /// even words that happen to still hold the pattern.
    ///
    /// # Safety
    /// Nothing should be running on this stack.
    pub unsafe fn high_water(&self) -> Option<usize> {
        if !TRACK_STACK_USAGE {
            return None;
        }

        let base: *mut u64 = self.bottom.as_mut_ptr();
        let words = Self::PAINT_SIZE / size_of::<u64>();

        // SAFETY: the range is mapped, aligned, and unused.
        Some(
            (0..words)
                .position(|i| unsafe { base.add(i).read_volatile() } != PAINT_PATTERN)
                .map_or(0, |first| Self::PAINT_SIZE - first * size_of::<u64>()),
        )
    }
}

/// A hart's slice of `STACK0`.
/// `BOOTSTACK_SIZE` bytes long, identity-mapped, with no guard page.
/// It runs `start`/`main`, then the scheduler loop forever, plus interrupts taken while idle.
#[derive(Debug)]
pub struct BootStack {
    bottom: VA,
}

impl BootStack {
    /// Calculates the `hart`'s slice of `STACK0`, mirroring the `sp` computation in `_entry`.
    pub fn for_hart(hart: usize) -> Self {
        Self {
            bottom: VA::new(&raw const STACK0 as usize + (hart * BOOTSTACK_SIZE)),
        }
    }

    /// Fills the bottom of the stack with a known pattern.
    ///
    /// # Safety
    /// The caller's frame must be above the margin, so painting doesn't overwrite live frames.
    pub unsafe fn paint_margin(&self) {
        let marker = 0u8;
        debug_assert!(
            &raw const marker as usize > self.bottom.as_usize() + STACK_MARGIN,
            "caller's frame is inside the margin, painting would overwrite live frames"
        );

        unsafe { fill(self.bottom, STACK_MARGIN) };
    }

    /// Returns whether the bottom `STACK_MARGIN` bytes of the stack still hold the pattern painted
    /// by `paint_margin()`.
    ///
    /// # Safety
    /// The margin must have been painted by `paint_margin()`, and interrupts must be disabled so no
    /// trap frame is written to this stack during the read.
    pub unsafe fn check_margin(&self) -> bool {
        unsafe { margin_intact(self.bottom) }
    }
}
