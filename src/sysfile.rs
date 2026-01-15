use crate::syscall::{SyscallArgs, SyscallError};

pub fn sys_dup(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_read(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_write(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_close(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_fstat(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_link(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_unlink(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_open(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_mkdir(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_mknod(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_chdir(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_exec(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_pipe(args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}
