use std::process::{Command, self};

fn main() {
    // read env variables that were set in build script
    let uefi_path = env!("UEFI_PATH");
    let bios_path = env!("BIOS_PATH");

    // choose whether to start the UEFI or BIOS image
    let uefi = true;

    let mut cmd = Command::new("qemu-system-x86_64");

    if uefi {
        println!("UEFI: {uefi_path}");
        cmd.arg("-bios").arg(ovmf_prebuilt::ovmf_pure_efi());
        cmd.arg("-drive").arg(format!("format=raw,file={uefi_path}"));
    } else {
        println!("BIOS: {bios_path}");
        cmd.arg("-drive").arg(format!("format=raw,file={bios_path}"));
    }

    cmd.arg("-device").arg("isa-debug-exit,iobase=0xf4,iosize=0x04");
    cmd.arg("-serial").arg("stdio");
    // cmd.arg("-display").arg("none");

    let mut qemu = cmd.spawn().unwrap();
    let status = qemu.wait().unwrap();

    process::exit(status.code().unwrap_or(-1));
}
