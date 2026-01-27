set confirm off
set architecture riscv:rv64
target remote 127.0.0.1:1234
symbol-file target/riscv64gc-unknown-none-elf/release/octopos
set disassemble-next-line auto
set riscv use-compressed-breakpoints yes
