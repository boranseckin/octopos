#![no_std]
#![feature(fn_align)]
#![feature(naked_functions)]
#![feature(negative_impls)]
#![feature(allocator_api)]
#![feature(alloc_error_handler)]
#![feature(const_refs_to_static)]
#![allow(unused)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::new_without_default)]

extern crate alloc;

pub mod console;
pub mod entry;
pub mod kalloc;
pub mod kernelvec;
pub mod memlayout;
pub mod param;
pub mod plic;
pub mod printf;
pub mod proc;
pub mod riscv;
pub mod spinlock;
pub mod start;
pub mod trampoline;
pub mod trap;
pub mod uart;
pub mod vm;
