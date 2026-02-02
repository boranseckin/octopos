#![no_std]
#![no_main]

#[unsafe(export_name = "main")]
extern "C" fn main() -> ! {
    kernel::main()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo<'_>) -> ! {
    kernel::panic_handler(info)
}
