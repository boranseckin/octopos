#![allow(dead_code)]

use core::arch::asm;
use core::ops::{Bound, RangeBounds};

use crate::spinlock::SpinLock;
use crate::sync::OnceLock;

const MULTIPLIER: u64 = 6364136223846793005;

/// Permuted Congruential Generator
/// xorshift then random rotation (XSH-RR)
///
/// state = state * MULTIPLIER + increment
///
/// xorshifted = ((state >> 18) ^ state) >> 27  // 32-bit value
/// rotation   = state >> 59                    // how much to rotate
/// output     = rotr32(xorshifted, rotation)
#[derive(Debug)]
struct PcgState {
    /// Current position in the sequence.
    /// Updated every time a new number is generated.
    /// Initialized from an entropy source.
    state: u64,
    increment: u64,
}

static RNG: OnceLock<SpinLock<PcgState>> = OnceLock::new();

impl PcgState {
    // https://en.wikipedia.org/wiki/Permuted_congruential_generator#Example_code
    fn step(&mut self) -> u32 {
        let mut x = self.state;
        let count = (x >> 59) as u32;

        self.state = x.wrapping_mul(MULTIPLIER).wrapping_add(self.increment);

        x ^= x >> 18;
        ((x >> 27) as u32).rotate_right(count)
    }
}

pub trait RandInt: Copy {
    const MIN: Self;
    const MAX: Self;
    fn to_u128(self) -> u128;
    fn from_u64(v: u64) -> Self;
}

macro_rules! impl_rand_int {
    ($target:ident) => {
        impl RandInt for $target {
            const MIN: Self = $target::MIN;

            const MAX: Self = $target::MAX;

            fn to_u128(self) -> u128 {
                self as u128
            }

            fn from_u64(v: u64) -> Self {
                v as Self
            }
        }
    };
}

impl_rand_int!(u8);
impl_rand_int!(u16);
impl_rand_int!(u32);
impl_rand_int!(u64);
impl_rand_int!(usize);

pub fn rand_u32() -> u32 {
    RNG.get().expect("rng not initialized").lock().step()
}

pub fn rand_u64() -> u64 {
    let mut rng = RNG.get().expect("rng not initialized").lock();
    let lo = rng.step();
    let hi = rng.step();
    (hi as u64) << 32 | lo as u64
}

pub fn rand_range<T, R>(range: R) -> T
where
    T: RandInt,
    R: RangeBounds<T>,
{
    let start = match range.start_bound() {
        Bound::Included(&v) => v.to_u128(),
        Bound::Excluded(&v) => v.to_u128() + 1,
        Bound::Unbounded => T::MIN.to_u128(),
    };

    let end = match range.end_bound() {
        Bound::Included(&v) => v.to_u128(),
        Bound::Excluded(&v) => v
            .to_u128()
            .checked_sub(1)
            .expect("rand_range called with an empty range"),
        Bound::Unbounded => T::MAX.to_u128(),
    };

    assert!(start <= end, "rand_range called with an empty range");

    let range_size = end - start + 1;
    let rand = rand_u64() as u128;
    let scaled = rand % range_size;
    T::from_u64((start + scaled) as u64)
}

pub fn rand_bytes(buf: &mut [u8]) {
    let mut rng = RNG.get().expect("rng not initialized").lock();

    let (chunks, remainder) = buf.as_chunks_mut::<4>();
    for chunk in chunks {
        chunk.copy_from_slice(&rng.step().to_le_bytes());
    }
    if !remainder.is_empty() {
        remainder.copy_from_slice(&rng.step().to_le_bytes()[..remainder.len()]);
    }
}

pub unsafe fn init() {
    let mut seed = 0u64;

    // seed register only produces 16-bit entropy at a time
    // so we need to read it 4 times to get a full 64-bit seed
    for _ in 0..4 {
        let entropy = loop {
            let reg: u32;
            unsafe { asm!("csrrw {}, seed, x0", out(reg) reg) };
            // entropy is only ready if status is ES16 (0b10)
            if reg >> 30 == 0b10 {
                break reg & 0xFFFF;
            }
        };

        seed = (seed << 16) | (entropy as u64);
    }

    // increment must be odd
    let increment = seed | 1;
    // start state as 1 and simulate one iteration to warm it up
    let state = 1u64.wrapping_mul(MULTIPLIER).wrapping_add(increment);

    RNG.initialize(|| Ok::<_, ()>(SpinLock::new(PcgState { state, increment }, "rng")));

    println!("rng  init");
}
