#![no_std]
#![no_main]

extern crate alloc;

use alloc::alloc::{alloc as raw_alloc, dealloc as raw_dealloc};
use core::alloc::Layout;
use core::hint::black_box;
use core::ptr;

use user::*;

/// Allocator unit: one 16-byte header.
const UNIT: usize = 16;

fn brk() -> usize {
    sbrk(0).expect("sbrk(0)")
}

fn bytes(size: usize) -> Layout {
    Layout::from_size_align(size, 1).unwrap()
}

/// Freed neighbours merge: after freeing many small blocks, one block of their combined size fits
/// in the same memory without growing the heap.
///
/// Must run first. On an empty heap, the small blocks are carved back to back from fresh sbrk
/// memory, so they cover one contiguous range.
fn test_free_blocks_merge() {
    const COUNT: usize = 64;
    /// 16 units with the header; 16 of these fill one 4 KiB grow exactly.
    const SMALL: usize = 15 * UNIT;
    /// All small blocks combined, minus the one header the large block needs.
    const LARGE: usize = COUNT * (SMALL + UNIT) - UNIT;

    let small = bytes(SMALL);
    let mut blocks = [ptr::null_mut(); COUNT];
    for block in blocks.iter_mut() {
        *block = unsafe { raw_alloc(small) };
        assert!(!block.is_null(), "small allocation failed");
    }
    let lowest = *blocks.iter().min().unwrap();

    for &block in &blocks {
        unsafe { raw_dealloc(block, small) };
    }
    let before = brk();

    let large = bytes(LARGE);
    let p = unsafe { raw_alloc(large) };
    assert!(!p.is_null(), "large allocation failed");
    assert_eq!(brk(), before, "heap grew: freed blocks did not merge");
    assert_eq!(
        p, lowest,
        "merged block must start at the lowest freed block"
    );

    unsafe { raw_dealloc(p, large) };
}

/// Box, Vec (with repeated growth) and String work through the global allocator.
fn test_collections() {
    let b = Box::new(0xdead_beef_u64);
    assert_eq!(*b, 0xdead_beef, "box value mismatch");

    let mut v = Vec::new();
    for i in 0..10_000u32 {
        v.push(i);
    }
    assert!(
        v.iter().enumerate().all(|(i, &x)| x == i as u32),
        "vec contents corrupted while growing"
    );

    let s = format!("{}-{}", "heap", 42);
    assert_eq!(s, "heap-42", "format! mismatch");

    let mut s = String::new();
    for _ in 0..100 {
        s.push_str("ab");
    }
    assert_eq!(s.len(), 200, "string length mismatch");
}

/// A freed block is reused: repeating the same allocation does not grow the heap.
fn test_reuse_without_growth() {
    let layout = bytes(1000);

    let p = unsafe { raw_alloc(layout) };
    assert!(!p.is_null(), "allocation failed");
    unsafe { raw_dealloc(p, layout) };
    let before = brk();

    for _ in 0..1000 {
        let p = unsafe { raw_alloc(layout) };
        assert!(!p.is_null(), "allocation failed");
        unsafe { raw_dealloc(p, layout) };
    }

    assert_eq!(brk(), before, "heap grew: freed blocks are not reused");
}

/// Every block is 16-byte aligned, which covers every alignment up to 16.
fn test_alignment() {
    for align in [1, 2, 4, 8, 16] {
        let mut blocks = [(ptr::null_mut(), bytes(1)); 64];

        for (i, block) in blocks.iter_mut().enumerate() {
            let layout = Layout::from_size_align(i + 1, align).unwrap();
            let p = unsafe { raw_alloc(layout) };
            assert!(!p.is_null(), "allocation failed");
            assert_eq!(p as usize % UNIT, 0, "block is not 16-byte aligned");
            *block = (p, layout);
        }

        for (p, layout) in blocks {
            unsafe { raw_dealloc(p, layout) };
        }
    }
}

/// Runs `layout` through the allocator for real. An allocation whose only use is a null check is
/// otherwise removed by LLVM and assumed to succeed.
fn alloc_escaped(layout: Layout) -> *mut u8 {
    black_box(unsafe { raw_alloc(layout) })
}

/// Alignments above 16 are not supported: null, not a misaligned block.
fn test_unsupported_alignment() {
    let layout = Layout::from_size_align(64, 2 * UNIT).unwrap();
    assert!(
        alloc_escaped(layout).is_null(),
        "alignment above 16 must return null"
    );
}

/// Live blocks never overlap: fill each block with its own byte, free every other one, allocate
/// smaller blocks into the holes, and check every block still holds its own byte.
fn test_no_overlap() {
    const COUNT: usize = 48;

    fn fill(p: *mut u8, layout: Layout, byte: u8) {
        unsafe { ptr::write_bytes(p, byte, layout.size()) };
    }

    fn check(p: *mut u8, layout: Layout, byte: u8) -> bool {
        unsafe { core::slice::from_raw_parts(p, layout.size()) }
            .iter()
            .all(|&b| b == byte)
    }

    let mut blocks = [(ptr::null_mut(), bytes(1), 0u8); COUNT];

    for (i, block) in blocks.iter_mut().enumerate() {
        let layout = bytes(1 + (i * 37) % 300);
        let p = unsafe { raw_alloc(layout) };
        assert!(!p.is_null(), "allocation failed");
        fill(p, layout, i as u8);
        *block = (p, layout, i as u8);
    }

    for (i, block) in blocks.iter_mut().enumerate().filter(|(i, _)| i % 2 == 0) {
        unsafe { raw_dealloc(block.0, block.1) };

        let layout = bytes(block.1.size() / 2 + 1);
        let p = unsafe { raw_alloc(layout) };
        assert!(!p.is_null(), "allocation failed");
        let byte = 0x80 | i as u8;
        fill(p, layout, byte);
        *block = (p, layout, byte);
    }

    for (p, layout, byte) in blocks {
        assert!(
            check(p, layout, byte),
            "block was overwritten by another block"
        );
        unsafe { raw_dealloc(p, layout) };
    }
}

/// After fork, parent and child have separate heaps: the child's writes and allocations do not
/// reach the parent, and the parent's allocator keeps working.
fn test_fork_isolation() {
    let mut v: Vec<u32> = (0..1000).collect();

    if fork().expect("fork") == 0 {
        v.iter_mut().for_each(|x| *x = 0);

        let extra: Vec<u64> = (0..5000).collect();
        assert_eq!(extra[4999], 4999, "child allocation corrupted");

        drop(extra);
        drop(v);
        exit(0);
    }

    let mut code = 0;
    wait(&mut code).expect("wait");
    assert_eq!(code, 0, "child failed");

    assert!(
        v.iter().enumerate().all(|(i, &x)| x == i as u32),
        "child's writes reached the parent"
    );

    v.push(1000);
    assert_eq!(v.len(), 1001, "parent allocation failed after fork");
}

/// A request too large for sbrk's isize returns null and leaves the heap alone, instead of
/// wrapping to a negative sbrk that would shrink it.
fn test_huge_request() {
    let before = brk();

    let layout = bytes(isize::MAX as usize);
    assert!(
        alloc_escaped(layout).is_null(),
        "huge request must return null"
    );
    assert_eq!(brk(), before, "huge request changed the heap size");

    let b = Box::new(7u8);
    assert_eq!(*b, 7, "allocation failed after huge request");
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_free_blocks_merge();
    test_collections();
    test_reuse_without_growth();
    test_alignment();
    test_unsupported_alignment();
    test_no_overlap();
    test_fork_isolation();
    test_huge_request();
}
