use core::sync::atomic::AtomicUsize;

/// To be set by `pci::init()`, if the device is found
pub static E1000_BASE: AtomicUsize = AtomicUsize::new(0);

pub unsafe fn init() {
    if E1000_BASE.load(core::sync::atomic::Ordering::SeqCst) == 0 {
        println!("e1000 not found");
        return;
    }

    println!("e1000 init");
}
