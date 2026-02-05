use core::fmt::Display;
use core::slice;

use crate::fs::Path;
use crate::log::Operation;
use crate::param::{MAXARG, USERSTACK};
use crate::proc::current_proc;
use crate::riscv::{PGSIZE, PTE_W, PTE_X, pg_round_up};
use crate::vm::VA;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecError {
    Alloc,
    Elf,
    Header,
    Read,
    Memory,
}

impl Display for ExecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ExecError::Alloc => write!(f, "allocation error"),
            ExecError::Elf => write!(f, "invalid elf file"),
            ExecError::Header => write!(f, "invalid program header"),
            ExecError::Read => write!(f, "read error"),
            ExecError::Memory => write!(f, "memory error"),
        }
    }
}

const ELF_MAGIC: u32 = 0x464C457F; // "\x7FELF" in little endian

#[repr(C)]
#[derive(Debug)]
/// File Header
struct ElfHeader {
    magic: u32,
    elf: [u8; 12],
    r#type: u16,
    machine: u16,
    version: u32,
    entry: u64,
    phoff: u64,
    shoff: u64,
    flags: u32,
    ehsize: u16,
    phentsize: u16,
    phnum: u16,
    shentsize: u16,
    shnum: u16,
    shstrndx: u16,
}

impl ElfHeader {
    pub const SIZE: usize = size_of::<Self>();

    pub fn from_bytes(bytes: &[u8]) -> Self {
        assert!(bytes.len() >= Self::SIZE);
        unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const Self) }
    }
}

#[repr(C)]
#[derive(Debug)]
/// Program Section Header
struct ProgramHeader {
    r#type: u32,
    flags: u32,
    offset: u64,
    vaddr: u64,
    paddr: u64,
    filesz: u64,
    memsz: u64,
    align: u64,
}

impl ProgramHeader {
    pub const SIZE: usize = size_of::<Self>();

    // Values for type
    pub const ELF_PROG_LOAD: u32 = 1;

    pub fn from_bytes(bytes: &[u8]) -> Self {
        assert!(bytes.len() >= Self::SIZE);
        unsafe { core::ptr::read_unaligned(bytes.as_ptr() as *const Self) }
    }

    fn get_perms(&self) -> usize {
        let mut perm = 0;
        if self.flags & 0x1 != 0 {
            perm = PTE_X;
        }
        if self.flags & 0x2 != 0 {
            perm |= PTE_W;
        }
        perm
    }
}

pub fn exec(path: &Path, argv: &[&str]) -> Result<usize, ExecError> {
    let proc = current_proc();
    let mut size = 0;

    let _op = Operation::begin();

    // open the executable file
    let Ok(mut inode) = log!(path.resolve()) else {
        err!(ExecError::Read);
    };

    let mut inner = inode.lock();

    // read the elf header
    let mut elf_buf = [0u8; ElfHeader::SIZE];
    match log!(inode.read(&mut inner, 0, &mut elf_buf, false)) {
        Ok(read) if read as usize == elf_buf.len() => {}
        _ => {
            inode.unlock_put(inner);
            err!(ExecError::Read);
        }
    }

    let elf = ElfHeader::from_bytes(&elf_buf);

    // make sure it's a valid elf file
    if elf.magic != ELF_MAGIC {
        inode.unlock_put(inner);
        err!(ExecError::Elf);
    }

    // create a new pagetable
    let Ok(mut pagetable) = log!(proc.create_pagetable()) else {
        inode.unlock_put(inner);
        err!(ExecError::Alloc);
    };

    // load program into memory
    let mut ph_buf = [0u8; ProgramHeader::SIZE];
    let mut offset = elf.phoff;

    for _ in 0..elf.phnum {
        match log!(inode.read(&mut inner, offset as u32, &mut ph_buf, false)) {
            Ok(read) if read as usize == ph_buf.len() => {}
            _ => {
                inode.unlock_put(inner);
                err!(ExecError::Memory);
            }
        }

        let ph = ProgramHeader::from_bytes(&ph_buf);
        offset += ProgramHeader::SIZE as u64;

        if ph.r#type != ProgramHeader::ELF_PROG_LOAD {
            continue;
        }

        if ph.memsz < ph.filesz
            || ph.vaddr.checked_add(ph.memsz).is_none()
            || !ph.vaddr.is_multiple_of(PGSIZE as u64)
        {
            pagetable.proc_free(size);
            inode.unlock_put(inner);
            err!(ExecError::Header);
        }

        size = match log!(pagetable.alloc(size, (ph.vaddr + ph.memsz) as usize, ph.get_perms())) {
            Ok(new_size) => new_size,
            Err(_) => {
                pagetable.proc_free(size);
                inode.unlock_put(inner);
                err!(ExecError::Alloc);
            }
        };

        if log!(pagetable.load_elf_segment(
            &mut inode,
            &mut inner,
            VA::from(ph.vaddr as usize),
            ph.offset as u32,
            ph.filesz as usize,
        ))
        .is_err()
        {
            pagetable.proc_free(size);
            inode.unlock_put(inner);
            err!(ExecError::Memory);
        }
    }

    inode.unlock_put(inner);
    drop(_op);

    let old_size = proc.data().size;

    // allocate some pages at the next page boundary.
    // make the first inaccessible as a stack guard.
    // use the rest as the user stack.
    size = pg_round_up(size);

    size = match log!(pagetable.alloc(size, size + (USERSTACK + 1) * PGSIZE, PTE_W)) {
        Ok(new_size) => new_size,
        Err(_) => {
            pagetable.proc_free(size);
            err!(ExecError::Alloc);
        }
    };

    if log!(pagetable.clear(VA::from(size - (USERSTACK + 1) * PGSIZE))).is_err() {
        pagetable.proc_free(size);
        err!(ExecError::Memory);
    }

    let mut sp = size;
    let stackbase = sp - USERSTACK * PGSIZE;

    // copy argument strings into new stack, remember their addresses in `ustack[]`
    let mut ustack = [0u64; MAXARG];
    let mut argc = 0;

    for &arg in argv.iter() {
        if argc >= MAXARG {
            pagetable.proc_free(size);
            err!(ExecError::Memory);
        }

        sp -= arg.len() + 1; // +1 for null terminator
        sp -= sp % 16; // riscv sp must be 16-byte aligned

        if sp < stackbase {
            pagetable.proc_free(size);
            err!(ExecError::Memory);
        }

        if log!(pagetable.copy_to(arg.as_bytes(), VA::from(sp))).is_err()
            || log!(pagetable.copy_to(&[0u8], VA::from(sp + arg.len()))).is_err()
        {
            pagetable.proc_free(size);
            err!(ExecError::Memory);
        }

        // save the address of the current argument
        ustack[argc] = sp as u64;
        argc += 1;
    }
    ustack[argc] = 0;

    // copy the pointer array to the user
    sp -= (argc + 1) * size_of::<u64>();
    sp -= sp % 16;

    let ustack_ptr = unsafe {
        slice::from_raw_parts(ustack.as_ptr() as *const u8, (argc + 1) * size_of::<u64>())
    };

    if sp < stackbase || log!(pagetable.copy_to(ustack_ptr, VA::from(sp))).is_err() {
        pagetable.proc_free(size);
        err!(ExecError::Memory);
    }

    // # Safety: we are the current proc
    let data = unsafe { proc.data_mut() };

    // save program name for debugging
    data.name.clear();
    data.name.insert_str(
        0,
        path.as_str()
            .rsplit_once("/")
            .unwrap_or(("", path.as_str()))
            .1,
    );

    // commit to the user image
    let old_pagetable = data.pagetable.replace(pagetable).unwrap();
    data.size = size;

    let trapframe = data.trapframe_mut();

    // a0 and a1 contain arguments to user main(argc, argv)
    // argc is returned via the system call return value at the end
    trapframe.a1 = sp;

    trapframe.epc = elf.entry as usize; // initial program counter = lib.c:start()
    trapframe.sp = sp; // initial stack pointer

    old_pagetable.proc_free(old_size);

    Ok(argc) // this end up in a0, the first argument to main(argc, argv)
}
