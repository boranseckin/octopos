use crate::syscall::{SyscallArgs, SyscallError};

pub fn sys_dup(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_read(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_write(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let addr = args.get_addr(1);
    let n = args.get_int(2);

    // TODO: fd

    crate::console::Console::write(addr, n as usize)
}

pub fn sys_close(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_fstat(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_link(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_unlink(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_open(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_mkdir(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_mknod(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_chdir(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_exec(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_pipe(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}
