# octopos

xv6 for RISC-V in Rust

## Usage

### Prerequisites

- Rust nightly toolchain (see `rust-toolchain.toml`)
- `qemu-system-riscv64`

### Build and Run

```bash
# Build kernel and user programs
cargo build --release

# Create and populate the filesystem image
qemu-img create target/fs.img 2G
./mkfs.sh

# Run in QEMU
cargo run --release
```

## Current State

The kernel boots, initializes all subsystems, and runs a userspace init process that forks and execs
programs from the filesystem. Currently at Stage 8 — exec and init are working, shell is not yet
implemented.

## Development Plan

### Stage 1: Boot & Hardware (done)

1. Entry point at 0x80000000 — per-CPU stack setup
2. Machine-mode start — privilege mode config, interrupt delegation, timer init
3. Supervisor-mode main — hart 0 initializes subsystems, other harts wait
4. Console/UART driver — serial I/O with interrupt-driven TX/RX
5. PLIC interrupt controller — external interrupt routing and claim/complete

### Stage 2: Memory Management (done)

1. Physical memory allocator — buddy allocator (`buddy-alloc` crate)
2. Sv39 page tables — 3-level page table walk, map, unmap
3. Kernel virtual memory (Kvm) — identity-map kernel, devices, trampoline
4. User virtual memory (Uvm) — per-process page tables

### Stage 3: Processes & Scheduling (done)

1. Process control blocks — fixed pool of 64 processes with spinlock-protected state
2. Trampoline & trap frames — user/kernel transition via shared trampoline page
3. Trap handling — user traps (syscall, interrupt, fault) and kernel traps
4. Context switch (`swtch`) — callee-saved register save/restore
5. Scheduler — round-robin scheduling with sleep/wakeup
6. Synchronization — spinlocks, `OnceLock`

### Stage 4: Syscalls & Process Management (done)

1. Syscall dispatcher — parse a7 register for syscall number
2. Console read/write — user-facing I/O
3. fork() — clone process, copy memory
4. wait() — wait for child exit, reparent logic
5. exit(), kill(), getpid()
6. sleep() — user-space sleep
7. sbrk() — grow/shrink process heap

### Stage 5: VirtIO & Block Layer (done)

1. VirtIO disk driver
2. Buffer cache — block caching layer
3. Disk interrupt handling
4. Sleep locks — blocking locks for long-held resources

### Stage 6: File System (done)

1. Logging layer — write-ahead logging for crash recovery
2. Superblock — filesystem metadata
3. Inode layer — on-disk inode structure, read/write
4. Directory layer — directory operations
5. Path name resolution
6. File descriptor abstraction and device table

### Stage 7: File Syscalls (done)

1. open(), close()
2. read(), write() (for files)
3. fstat()
4. link(), unlink()
5. mkdir(), chdir()
6. mknod() — device files
7. dup()

### Stage 8: exec & User Space (in progress)

1. exec() syscall — load ELF binary, set up new address space
2. Cargo workspace restructuring — kernel/user crate split, per-crate build scripts and linker scripts
3. User space crate — syscall wrappers, panic handler
4. /init program — first userspace process (opens console, forks and execs shell)
5. Shell — user shell (not yet implemented)

### Stage 9: Pipes & Advanced Features

1. pipe() syscall
2. Console as device file
3. Multi-hart scheduling
