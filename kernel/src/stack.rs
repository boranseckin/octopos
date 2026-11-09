//! Stack overflow tripwires and usage measurement for kernel and boot stacks.
//!
//! A stack grows down from its top, and every frame the code writes overwrites what was below
//! `sp`. So a range painted with `PAINT_PATTERN` before use shows afterwards how deep the stack
//! went.
//!
//! Two uses share this:
//! - Tripwire (always on): only the bottom `STACK_MARGIN` bytes are painted, and the scheduler
//!   checks them at points where it is not running on that stack. A stack that came within the
//!   margin of overflowing panics there with a message, instead of hanging later on its guard
//!   page (kernel stacks) or silently corrupting the next hart's slice (boot stacks).
//! - Measurement (`TRACK_STACK_USAGE`): the whole kstack is painted, and `high_water` reports how
//!   much of it a process used by the time it exits.

use crate::param::{MAX_KERNEL_STACK_BUFFER, NKSTACK_PAGES, TRACK_STACK_USAGE};
use crate::riscv::PGSIZE;
use crate::vm::VA;

/// Written into unused stack as a marker.
/// Spells out `STACK_PA` in ASCII.
const PAINT_PATTERN: u64 = 0x5354_4143_4B5F_5041;

/// Bytes at the bottom of each stack that must stay untouched.
pub const STACK_MARGIN: usize = MAX_KERNEL_STACK_BUFFER;

/// How much of each kstack is painted: the whole stack when measuring, otherwise only the tripwire
/// margin. Kernel stacks only; boot stacks always paint `STACK_MARGIN`.
pub const PAINT_SIZE: usize = if TRACK_STACK_USAGE {
    NKSTACK_PAGES * PGSIZE
} else {
    STACK_MARGIN
};

/// Fills `len` bytes upwards from `bottom` with `PAINT_PATTERN`.
///
/// # Safety
/// `bottom..bottom + len` must be mapped, 8-byte aligned, and not in use by any running code.
pub unsafe fn paint(bottom: VA, len: usize) {
    debug_assert_eq!(len % size_of::<u64>(), 0, "paint length is whole words");

    let base: *mut u64 = bottom.as_mut_ptr();
    let words = len / size_of::<u64>();

    // SAFETY: the range is mapped, aligned, and unused.
    (0..words).for_each(|i| unsafe {
        base.add(i).write_volatile(PAINT_PATTERN);
    });
}

/// Returns whether the bottom `STACK_MARGIN` bytes above `bottom` still hold the pattern.
///
/// # Safety
/// `bottom..bottom + STACK_MARGIN` must be mapped and 8-byte aligned, and must have been painted
/// with `paint`. The stack must not be running while it is checked.
pub unsafe fn check_margin(bottom: VA) -> bool {
    let base: *mut u64 = bottom.as_mut_ptr();

    // SAFETY: the range is mapped, aligned, and unused.
    (0..STACK_MARGIN / 8).all(|i| unsafe { base.add(i).read_volatile() } == PAINT_PATTERN)
}

/// Returns how many bytes of the painted `len`-byte range were used, measured from the top.
///
/// Scans up from the bottom to the first overwritten word. Everything above it counts as used,
/// even words that happen to still hold the pattern.
///
/// # Safety
/// `bottom..bottom + len` must be mapped, 8-byte aligned, and not in use by any running code.
pub unsafe fn high_water(bottom: VA, len: usize) -> usize {
    let base: *mut u64 = bottom.as_mut_ptr();
    let words = len / size_of::<u64>();

    // SAFETY: the range is mapped, aligned, and unused.
    (0..words)
        .position(|i| unsafe { base.add(i).read_volatile() } != PAINT_PATTERN)
        .map_or(0, |first| len - first * size_of::<u64>())
}
