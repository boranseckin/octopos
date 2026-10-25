#![no_std]
#![no_main]

#[global_allocator]
static GLOBAL: kernel::KernelAllocator = kernel::KernelAllocator;

#[unsafe(export_name = "main")]
fn main() -> ! {
    kernel::main()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    kernel::panic_handler(info)
}
