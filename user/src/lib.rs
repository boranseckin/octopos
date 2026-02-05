#![no_std]

use core::panic::PanicInfo;

#[macro_use]
mod io;
mod args;
mod syscall;

pub use kernel::abi::*;

pub use args::*;
pub use io::*;
pub use syscall::*;

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
extern "C" fn _start() -> ! {
    unsafe {
        let args = Args::from_stack();
        main(args);
        exit(0);
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    eprintln!("! {}", info);
    exit(1)
}
