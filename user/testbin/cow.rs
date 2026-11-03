#![no_std]
#![no_main]

use user::env::Args;
use user::process::{exit, fork, sbrk, wait};

/// Verify parent and child see independent copies of the same page after a write.
fn test_basic_cow() {
    let mut val: u32 = 42;

    if fork().expect("fork") == 0 {
        val = 99;
        assert_eq!(val, 99);
        exit(0);
    }

    let mut code = 0;
    wait(&mut code).expect("wait");
    assert_eq!(code, 0, "child did not exit cleanly");

    assert_eq!(val, 42, "parent saw child's write");
}

/// Fork a chain of children, each modifying the same variable.
/// Every generation must see its own value, not a sibling's.
fn test_fork_chain() {
    let mut val = 0;

    for i in 0..4 {
        if fork().expect("fork") == 0 {
            val = i;
            assert_eq!(val, i);
            exit(0);
        }

        let mut code = 0;
        wait(&mut code).expect("wait");
        assert_eq!(code, 0, "child did not exit cleanly");
    }

    assert_eq!(val, 0, "parent's val was modified");
}

/// Write a full page of data before forking, then verify child gets the right
/// snapshot and parent is not affected by child's overwrite.
fn test_full_page() {
    let base = sbrk(0x1000).expect("grow");
    let page = base as *mut u8;

    for i in 0..0x1000 {
        unsafe { *page.add(i) = (i & 0xFF) as u8 };
    }

    if fork().expect("fork") == 0 {
        // overwrite entire page in child
        for i in 0..0x1000 {
            unsafe { *page.add(i) = 0xFF };
        }
        exit(0);
    }

    let mut code = 0;
    wait(&mut code).expect("wait");
    assert_eq!(code, 0, "child did not exit cleanly");

    // parent's page must still hold the original pattern
    for i in 0..0x1000 {
        assert_eq!(
            unsafe { *page.add(i) },
            (i & 0xFF) as u8,
            "parent page corrupted at offset {}",
            i
        );
    }
}

/// Verify that writing to a page in the child process triggers lazy allocation, and does not affect
/// the parent process's page.
fn test_cow_lazy() {
    let base = sbrk(0x1000).expect("grow");

    if fork().expect("fork") == 0 {
        // write to the page in child, which should trigger lazy allocation
        unsafe { *(base as *mut u8) = 42 };
        assert_eq!(
            unsafe { *(base as *mut u8) },
            42,
            "child page should have been lazily allocated and modified"
        );
        exit(0);
    }

    let mut code = 0;
    wait(&mut code).expect("wait");
    assert_eq!(code, 0, "child did not exit cleanly");

    // parent's page should still be zero-initialized
    assert_eq!(
        unsafe { *(base as *mut u8) },
        0,
        "parent page should be zero-initialized"
    );
}

#[unsafe(no_mangle)]
fn main(_args: Args) {
    test_basic_cow();
    test_fork_chain();
    test_full_page();
    test_cow_lazy();
}
