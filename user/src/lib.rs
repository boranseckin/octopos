#![no_std]

pub extern crate alloc;

#[macro_use]
pub mod io;
pub mod dns;
pub mod env;
pub mod fs;
pub mod net;
pub mod prelude;
pub mod process;
pub mod random;
pub mod syscall;

mod heap;
mod line;

use crate::env::Args;
use crate::process::exit;

unsafe extern "Rust" {
    /// The entry point for user programs.
    /// This function is called by the user entry `_start()` with the command-line arguments passed in as `args`.
    /// User binaries must define this function with the same signature and with `no_mangle` attribute.
    fn main(args: Args);
}

/// The entry point for user programs.
/// This function is mapped to the `.text.entry` section, which is the entry point for user processes.
///
/// Whichever elf binary is currently loaded in the memory, this function will jump to that binary's `main()` function.
/// Before the jump happens, the command-line arguments are extracted from the stack and passed to `main()`.
/// After `main()` returns, the process exits with a status code of 0.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
fn _start() -> ! {
    unsafe {
        let args = Args::from_stack();
        main(args);
        exit(0);
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    eprintln!("! {}", info);
    exit(1)
}
