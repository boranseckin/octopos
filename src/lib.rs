#![no_std]
#![feature(fn_align)]
#![feature(naked_functions)]
#![feature(negative_impls)]
#![feature(asm_const)]
#![feature(alloc_error_handler)]
#![feature(const_refs_to_static)]
#![allow(unused)]
#![allow(clippy::missing_safety_doc)]

extern crate alloc;

pub mod console;
pub mod entry;
pub mod kalloc;
pub mod kernelvec;
pub mod memlayout;
pub mod param;
pub mod printf;
pub mod proc;
pub mod riscv;
pub mod spinlock;
pub mod start;
pub mod trampoline;
pub mod trap;
pub mod uart;
