use core::mem;
use core::slice;

use alloc::string::String;
use alloc::vec::Vec;

use crate::abi::OpenFlag;
use crate::exec::exec;
use crate::file::{FILE_TABLE, File, FileType};
use crate::fs::{Directory, Inode, InodeType, Path};
use crate::log::Operation;
use crate::param::{MAXARG, MAXPATH, NDEV};
use crate::pipe::Pipe;
use crate::proc::current_proc_and_data_mut;
use crate::riscv::PGSIZE;
use crate::syscall::{SyscallArgs, SyscallError};
use crate::vm::VA;

/// Allocates a file descriptor for the give file.
/// Takes over file reference from caller on success.
fn fd_alloc(file: File) -> Result<usize, SyscallError> {
    let (_proc, data) = current_proc_and_data_mut();

    for (fd, open_file) in data.open_files.iter_mut().enumerate() {
        if open_file.is_none() {
            *open_file = Some(file);
            return Ok(fd);
        }
    }

    err!(SyscallError::FetchArgument)
}

pub fn sys_dup(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let (_, mut file) = try_log!(args.get_file(0));
    let fd = try_log!(fd_alloc(file.clone()));
    file.dup();
    Ok(fd)
}

pub fn sys_read(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let addr = args.get_addr(1);
    let n = args.get_int(2);
    let (_, file) = try_log!(args.get_file(0));
    log!(file.read(addr, n as usize))
}

pub fn sys_write(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let addr = args.get_addr(1);
    let n = args.get_int(2);
    let (_, mut file) = try_log!(args.get_file(0));
    log!(file.write(addr, n as usize))
}

pub fn sys_close(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let (fd, mut file) = try_log!(args.get_file(0));

    let (_proc, data) = current_proc_and_data_mut();

    data.open_files[fd] = None;
    file.close();

    Ok(0)
}

pub fn sys_fstat(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let addr = args.get_addr(1);
    let (_, file) = try_log!(args.get_file(0));
    match log!(file.stat(addr)) {
        Ok(_) => Ok(0),
        Err(_) => err!(SyscallError::File("sys_fstat")),
    }
}

pub fn sys_link(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let old = try_log!(args.fetch_string(args.get_addr(0), MAXPATH));
    let new = try_log!(args.fetch_string(args.get_addr(1), MAXPATH));

    let _op = Operation::begin();

    // get the inode of the old
    let Ok(old_inode) = log!(Path::new(&old).resolve()) else {
        err!(SyscallError::File("sys_link resolve"))
    };

    let mut old_inner = old_inode.lock();

    // make sure it is not a directory
    if old_inner.r#type == InodeType::Directory {
        old_inode.unlock_put(old_inner);
        err!(SyscallError::File("sys_link directory"));
    }

    // increment number of links pointing to the inode
    old_inner.nlink += 1;
    old_inode.update(&old_inner);
    old_inode.unlock(old_inner);

    // after incrementing nlink, failures must goto `bad`
    let result = (|| {
        // get the inode of the new's parent
        let (parent, name) = match log!(Path::new(&new).resolve_parent()) {
            Ok(v) => v,
            Err(_) => err!(SyscallError::File("sys_link resolve parent")),
        };

        // make sure they are in the same device
        if parent.dev != old_inode.dev {
            err!(SyscallError::File("sys_link device mismatch"));
        }

        let mut parent_inner = parent.lock();

        // add the inode to the new's parent
        if log!(Directory::link(
            &parent,
            &mut parent_inner,
            name,
            old_inode.inum as u16
        ))
        .is_err()
        {
            parent.unlock_put(parent_inner);
            err!(SyscallError::File("sys_link link"));
        }

        parent.unlock_put(parent_inner);
        Ok(0)
    })();

    // bad
    if result.is_err() {
        let mut old_inner = old_inode.lock();
        old_inner.nlink -= 1;
        old_inode.update(&old_inner);
        old_inode.unlock(old_inner);
    }

    old_inode.put();

    result
}

pub fn sys_unlink(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let path = try_log!(args.fetch_string(args.get_addr(0), MAXPATH));

    let _op = Operation::begin();

    // get the parent inode and name
    let Ok((parent, name)) = log!(Path::new(&path).resolve_parent()) else {
        err!(SyscallError::File("sys_unlink resolve parent"));
    };

    let mut parent_inner = parent.lock();

    // cannot unlink `.` or `..`
    if name == "." || name == ".." {
        parent.unlock_put(parent_inner);
        err!(SyscallError::File("sys_unlink dot"));
    }

    // find the inode in the parent's directory entry
    let Ok(Some((offset, inode))) = log!(Directory::lookup(&parent, &mut parent_inner, name))
    else {
        parent.unlock_put(parent_inner);
        err!(SyscallError::File("sys_unlink lookup"));
    };

    let mut inode_inner = inode.lock();

    assert!(inode_inner.nlink >= 1, "unlink nlink < 1");

    // if the inode is a directory and it is not empty, cannot unlink
    if inode_inner.r#type == InodeType::Directory && !Directory::is_empty(&inode, &mut inode_inner)
    {
        inode.unlock_put(inode_inner);
        parent.unlock_put(parent_inner);
        err!(SyscallError::File("sys_unlink not empty"));
    }

    // replace the directory entry with an empty one
    let dir = Directory::new_empty();
    match log!(parent.write(&mut parent_inner, offset, dir.as_bytes(), false)) {
        Ok(write) => {
            assert_eq!(write, Directory::SIZE as u32, "unlink write");
        }
        Err(_) => {
            parent.unlock_put(parent_inner);
            err!(SyscallError::File("sys_unlink write"))
        }
    }

    // if it is a directory, decrement parent's link count
    if inode_inner.r#type == InodeType::Directory {
        parent_inner.nlink -= 1;
        parent.update(&parent_inner);
    }
    parent.unlock_put(parent_inner);

    // decrement the inode's link count
    inode_inner.nlink -= 1;
    inode.update(&inode_inner);
    inode.unlock_put(inode_inner);

    Ok(0)
}

pub fn sys_open(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let o_mode = args.get_int(1) as usize;
    let path = try_log!(args.fetch_string(args.get_addr(0), MAXPATH));
    let path = Path::new(&path);

    let _op = Operation::begin();

    let (mut inode, mut inode_inner);

    // either create a new file or find the file from the path
    if (o_mode & OpenFlag::CREATE) != 0 {
        (inode, inode_inner) = match log!(Inode::create(&path, InodeType::File, 0, 0)) {
            Ok(i) => i,
            Err(_) => {
                err!(SyscallError::File("sys_open create"))
            }
        };
    } else {
        inode = match log!(path.resolve()) {
            Ok(i) => i,
            Err(_) => {
                err!(SyscallError::File("sys_open resolve"));
            }
        };

        inode_inner = inode.lock();

        // if it is a directory, cannot open with write mode
        if inode_inner.r#type == InodeType::Directory && o_mode != OpenFlag::READ_ONLY {
            inode.unlock_put(inode_inner);
            err!(SyscallError::File("sys_open directory write"));
        }
    }

    // cannot open device out of range
    if inode_inner.r#type == InodeType::Device && inode_inner.major >= NDEV as u16 {
        inode.unlock_put(inode_inner);
        err!(SyscallError::File("sys_open device major"));
    }

    // allocate a file structure and a file descriptor
    let (fd, file) = match log!(File::alloc()) {
        Ok(mut file) => match log!(fd_alloc(file.clone())) {
            Ok(fd) => (fd, file),
            Err(e) => {
                // if err here, we must also close the file
                file.close();
                inode.unlock_put(inode_inner);
                return Err(e);
            }
        },
        Err(_) => {
            inode.unlock_put(inode_inner);
            err!(SyscallError::File("sys_open alloc"));
        }
    };

    let mut file_inner = FILE_TABLE.inner[file.id].lock();
    if inode_inner.r#type == InodeType::Device {
        file_inner.r#type = FileType::Device {
            inode: inode.clone(),
            major: inode_inner.major,
        };
    } else {
        file_inner.r#type = FileType::Inode {
            inode: inode.clone(),
        };
        file_inner.offset = 0;
    }
    file_inner.readable = (o_mode & OpenFlag::WRITE_ONLY) == 0;
    file_inner.writeable =
        (o_mode & OpenFlag::WRITE_ONLY) != 0 || (o_mode & OpenFlag::READ_WRITE != 0);

    if (o_mode & OpenFlag::TRUNCATE) != 0 && inode_inner.r#type == InodeType::File {
        inode.trunc(&mut inode_inner);
    }

    inode.unlock(inode_inner);

    Ok(fd)
}

pub fn sys_mkdir(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let _op = Operation::begin();

    let path = try_log!(args.fetch_string(args.get_addr(0), MAXPATH));

    let Ok((inode, inode_inner)) =
        log!(Inode::create(&Path::new(&path), InodeType::Directory, 0, 0))
    else {
        err!(SyscallError::File("sys_mkdir create"));
    };

    inode.unlock_put(inode_inner);

    Ok(0)
}

pub fn sys_mknod(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let _op = Operation::begin();

    let major = args.get_int(1) as u16;
    let minor = args.get_int(2) as u16;
    let path = try_log!(args.fetch_string(args.get_addr(0), MAXPATH));

    let Ok((inode, inner)) = log!(Inode::create(
        &Path::new(&path),
        InodeType::Device,
        major,
        minor
    )) else {
        err!(SyscallError::File("sys_mknod create"));
    };

    inode.unlock_put(inner);

    Ok(0)
}

pub fn sys_chdir(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let (_proc, data) = current_proc_and_data_mut();

    let _op = Operation::begin();

    let path = try_log!(args.fetch_string(args.get_addr(0), MAXPATH));

    let Ok(inode) = log!(Path::new(&path).resolve()) else {
        err!(SyscallError::File("sys_chdir resolve"));
    };

    let inner = inode.lock();

    if inner.r#type != InodeType::Directory {
        inode.unlock_put(inner);
        err!(SyscallError::File("sys_chdir not directory"));
    }

    inode.unlock(inner);

    let old_cwd = mem::replace(&mut data.cwd, inode);
    old_cwd.put();

    Ok(0)
}

pub fn sys_exec(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let uargv = args.get_addr(1);

    let path = try_log!(args.fetch_string(args.get_addr(0), MAXPATH));
    let path = Path::new(&path);

    let (_proc, data) = current_proc_and_data_mut();

    let mut argv_bufs: Vec<String> = Vec::with_capacity(MAXARG);

    for i in 0..MAXARG {
        // fetch pointer argv[i] from user space
        let mut uarg: usize = 0;
        let dst = unsafe {
            slice::from_raw_parts_mut(
                &mut uarg as *mut usize as *mut u8,
                core::mem::size_of::<usize>(),
            )
        };
        if log!(
            data.pagetable_mut()
                .copy_from(uargv + i * size_of::<usize>(), dst)
        )
        .is_err()
        {
            err!(SyscallError::File("sys_exec copy in"));
        }

        if uarg == 0 {
            break; // NULL terminator
        }

        // fetch string from user space
        let s = try_log!(args.fetch_string(VA::from(uarg), PGSIZE));
        argv_bufs.push(s);
    }

    let argv: Vec<&str> = argv_bufs.iter().map(|s| s.as_str()).collect::<Vec<_>>();

    log!(exec(&path, &argv)).map_err(|_| SyscallError::File("sys_exec exec"))
}

pub fn sys_pipe(args: &SyscallArgs) -> Result<usize, SyscallError> {
    // user pointer to array of two integers
    let fd_array = args.get_addr(0);

    let (_proc, data) = current_proc_and_data_mut();

    let Ok((mut read, mut write)) = log!(Pipe::alloc()) else {
        err!(SyscallError::File("sys_pipe alloc"));
    };

    let Ok(fd0) = log!(fd_alloc(read.clone())) else {
        read.close();
        write.close();
        err!(SyscallError::File("sys_pipe fd_alloc"));
    };

    let Ok(fd1) = log!(fd_alloc(write.clone())) else {
        data.open_files[fd0] = None;
        read.close();
        write.close();
        err!(SyscallError::File("sys_pipe fd_alloc"));
    };

    let pagetable = data.pagetable_mut();

    if log!(pagetable.copy_to(&fd0.to_le_bytes(), fd_array)).is_err()
        || log!(pagetable.copy_to(&fd1.to_le_bytes(), fd_array + size_of_val(&fd1))).is_err()
    {
        data.open_files[fd0] = None;
        data.open_files[fd1] = None;
        read.close();
        write.close();
        err!(SyscallError::File("sys_pipe copy_to"));
    }

    Ok(0)
}
