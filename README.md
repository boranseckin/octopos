# octopos

xv6 for RISC-V in Rust

## Usage

### Prerequisites

- Rust nightly toolchain
- qemu-system-riscv64

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

### Debugging

The QEMU runner in `.cargo/config.toml` includes `-s`, which always opens a GDB server on
`tcp::1234`. To halt the kernel at startup and wait for a debugger to attach, add `-S` to the
runner flags, then connect from a second terminal:

```bash
cargo build          # build with debug info
cargo run            # QEMU starts frozen, waiting for GDB

# in a second terminal:
riscv64-elf-gdb      # .gdbinit connects to port 1234 and loads symbols automatically
```

## Current State

The kernel boots, initializes all subsystems, and runs a full userspace environment including a
shell with pipes, redirections, and background jobs. All planned stages are complete.

## Development Plan

A kernel's subsystems have deep interdependencies, making it non-trivial to find an order in which
they can be built incrementally. This is the sequence I followed, though stubs and `todo!()`s were
often needed to break circular dependencies.

### Stage 1: Boot & Hardware

1. Entry point at 0x80000000 — per-CPU stack setup
2. Machine-mode start — privilege mode config, interrupt delegation, timer init
3. Supervisor-mode main — hart 0 initializes subsystems, other harts wait
4. Console/UART driver — polling TX/RX; UART hardware configured for interrupts
5. PLIC interrupt controller — external interrupt routing and claim/complete

### Stage 2: Memory Management

1. Physical memory allocator — buddy allocator (`buddy-alloc` crate)
2. Sv39 page tables — 3-level page table walk, map, unmap
3. Kernel virtual memory (Kvm) — identity-map kernel, devices, trampoline
4. User virtual memory (Uvm) — per-process page tables

### Stage 3: Processes & Scheduling

1. Synchronization — spinlocks, `OnceLock`
2. Process control blocks — fixed pool of 64 processes with spinlock-protected state
3. Trampoline & trap frames — user/kernel transition via shared trampoline page
4. Trap handling — user traps (syscall, interrupt, fault) and kernel traps
5. Context switch (`swtch`) — callee-saved register save/restore
6. Scheduler — round-robin scheduling with sleep/wakeup

### Stage 4: Syscalls & Process Management

1. Syscall dispatcher — parse a7 register for syscall number
2. Console read/write — user-facing I/O with interrupt-driven RX
3. fork() — clone process, copy memory
4. wait() — wait for child exit, reparent logic
5. exit(), kill(), getpid()
6. sleep() — user-space sleep
7. uptime() — return elapsed timer ticks since boot
8. sbrk() — grow/shrink process heap

### Stage 5: VirtIO & Block Layer

1. VirtIO disk driver
2. Buffer cache — block caching layer
3. Disk interrupt handling
4. Sleep locks — non-blocking locks that yield the CPU while waiting, for long-held resources

### Stage 6: File System

1. Logging layer — write-ahead logging for crash recovery
2. Superblock — filesystem metadata
3. Inode layer — on-disk inode structure, read/write
4. Directory layer — directory operations
5. Path name resolution
6. File descriptor abstraction and device table

### Stage 7: File Syscalls

1. open(), close() — open a file by path, release a file descriptor
2. read(), write() — read/write file data by file descriptor
3. fstat() — query file metadata (type, size, inode number)
4. link(), unlink() — create/remove a directory entry for an inode
5. mkdir(), chdir() — create a directory, change working directory
6. mknod() — create a special file bound to a device major/minor number
7. dup() — duplicate a file descriptor to the lowest available slot

### Stage 8: exec & User Space

1. exec() syscall — load ELF binary, set up new address space
2. Cargo workspace restructuring — kernel/user crate split, per-crate build scripts and linker scripts
3. User space crate — syscall wrappers, panic handler
4. /init program — first userspace process (opens console, forks and execs shell)
5. Shell — pipes, redirections, background jobs, built-ins (cd, exit)

### Stage 9: Pipes & Advanced Features

1. pipe() — create a unidirectional channel, returning a read/write file descriptor pair
2. Console as device file
3. Multi-hart scheduling
