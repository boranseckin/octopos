use crate::file::File;
use crate::proc::CPU_POOL;
use crate::syscall::{SyscallArgs, SyscallError};

/// Allocates a file descriptor for the give file.
/// Takes over file reference from caller on success.
fn fd_alloc(file: File) -> Result<usize, SyscallError> {
    let proc = CPU_POOL.current_proc().unwrap();
    let open_files = unsafe { &mut proc.data_mut().open_files };

    for (fd, open_file) in open_files.iter_mut().enumerate() {
        if open_file.is_none() {
            *open_file = Some(file);
            return Ok(fd);
        }
    }

    Err(SyscallError::FetchError)
}

pub fn sys_dup(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let (_, mut file) = args.get_file(0)?;
    let fd = fd_alloc(file)?;

    file.dup();

    Ok(fd)
}

pub fn sys_read(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let addr = args.get_addr(1);
    let n = args.get_int(2);
    let (_, file) = args.get_file(0)?;
    file.read(addr, n as usize).or(Err(SyscallError::ReadError))
}

pub fn sys_write(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let addr = args.get_addr(1);
    let n = args.get_int(2);
    let (_, mut file) = args.get_file(0)?;
    file.write(addr, n as usize)
        .or(Err(SyscallError::WriteError))
}

pub fn sys_close(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let (fd, mut file) = args.get_file(0)?;

    let proc = CPU_POOL.current_proc().unwrap();
    let open_files = unsafe { &mut proc.data_mut().open_files };

    open_files[fd] = None;
    file.close();

    Ok(0)
}

pub fn sys_fstat(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let addr = args.get_addr(1);
    let (_, file) = args.get_file(0)?;
    file.stat(addr).or(Err(SyscallError::StatError))?;
    Ok(0)
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
