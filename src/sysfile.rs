use core::mem;

use crate::file::{FILE_TABLE, File, FileType};
use crate::fs::{Directory, Inode, InodeType, Path};
use crate::log;
use crate::param::{MAXPATH, NDEV};
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

    Err(SyscallError::Fetch)
}

pub fn sys_dup(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let (_, mut file) = args.get_file(0)?;
    let fd = fd_alloc(file.clone())?;
    file.dup();
    Ok(fd)
}

pub fn sys_read(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let addr = args.get_addr(1);
    let n = args.get_int(2);
    let (_, file) = args.get_file(0)?;
    file.read(addr, n as usize).or(Err(SyscallError::Read))
}

pub fn sys_write(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let addr = args.get_addr(1);
    let n = args.get_int(2);
    let (_, mut file) = args.get_file(0)?;
    file.write(addr, n as usize).or(Err(SyscallError::Write))
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
    file.stat(addr).or(Err(SyscallError::Stat))?;
    Ok(0)
}

pub fn sys_link(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let old = args.fetch_string(args.get_addr(0), MAXPATH)?;
    let new = args.fetch_string(args.get_addr(1), MAXPATH)?;

    log::begin_op();

    // get the inode of the old
    let old_inode = Path::new(&old).resolve().map_err(|_| {
        log::end_op();
        SyscallError::Link
    })?;

    let mut old_inner = old_inode.lock();

    // make sure it is not a directory
    if old_inner.r#type == InodeType::Directory {
        old_inode.unlock_put(old_inner);
        log::end_op();
        return Err(SyscallError::Link);
    }

    // increment number of links pointing to the inode
    old_inner.nlink += 1;
    old_inode.update(&old_inner);
    old_inode.unlock(old_inner);

    // after incrementing nlink, failures must goto `bad`
    let result = 'bad: {
        // get the inode of the new's parent
        let (parent, name) = match Path::new(&new).resolve_parent() {
            Ok(v) => v,
            Err(_) => break 'bad Err(SyscallError::Link),
        };

        // make sure they are in the same device
        if parent.dev != old_inode.dev {
            break 'bad Err(SyscallError::Link);
        }

        let mut parent_inner = parent.lock();

        // add the inode to the new's parent
        if Directory::link(&parent, &mut parent_inner, name, old_inode.inum as u16).is_err() {
            parent.unlock_put(parent_inner);
            break 'bad Err(SyscallError::Link);
        }

        parent.unlock_put(parent_inner);
        Ok(0)
    };

    // bad
    if result.is_err() {
        let mut old_inner = old_inode.lock();
        old_inner.nlink -= 1;
        old_inode.update(&old_inner);
        old_inode.unlock(old_inner);
    }

    old_inode.put();

    log::end_op();

    result
}

pub fn sys_unlink(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let path = args.fetch_string(args.get_addr(0), MAXPATH)?;

    log::begin_op();

    // get the parent inode and name
    let (parent, name) = Path::new(&path).resolve_parent().map_err(|_| {
        log::end_op();
        SyscallError::Unlink
    })?;

    let mut parent_inner = parent.lock();

    let result = 'bad: {
        // cannot unlink `.` or `..`
        if name == "." || name == ".." {
            break 'bad Err(SyscallError::Unlink);
        }

        // find the inode in the parent's directory entry
        let (offset, inode) = match Directory::lookup(&parent, &mut parent_inner, name) {
            Ok(v) => v,
            Err(_) => {
                break 'bad Err(SyscallError::Unlink);
            }
        };

        let mut inode_inner = inode.lock();

        assert!(inode_inner.nlink >= 1, "unlink nlink < 1");

        // if the inode is a directory and it is not empty, cannot unlink
        if inode_inner.r#type == InodeType::Directory
            && !Directory::is_empty(&inode, &mut inode_inner)
        {
            inode.unlock_put(inode_inner);
            break 'bad Err(SyscallError::Unlink);
        }

        // replace the directory entry with an empty one
        let dir = Directory::new_empty();
        match parent.write(&mut parent_inner, offset, dir.as_bytes(), false) {
            Ok(write) => {
                assert_eq!(write, Directory::SIZE as u32, "unlink write");
            }
            Err(_) => break 'bad Err(SyscallError::Unlink),
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

        log::end_op();

        return Ok(0);
    };

    // bad
    if result.is_err() {
        parent.unlock_put(parent_inner);
        log::end_op();
    }

    result // this is only err
}

pub fn sys_open(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let o_mode = args.get_int(1) as i32;
    let path = args.fetch_string(args.get_addr(0), MAXPATH)?;
    let path = Path::new(&path);

    log::begin_op();

    let (mut inode, mut inode_inner);

    // either create a new file or find the file from the path
    if (o_mode & File::O_CREATE) != 0 {
        (inode, inode_inner) = Inode::create(&path, InodeType::File, 0, 0).map_err(|_| {
            log::end_op();
            SyscallError::Open
        })?;
    } else {
        inode = path.resolve().map_err(|_| {
            log::end_op();
            SyscallError::Open
        })?;

        inode_inner = inode.lock();

        // if it is a directory, cannot open with write mode
        if inode_inner.r#type == InodeType::Directory && o_mode != File::O_RDONLY {
            inode.unlock_put(inode_inner);
            log::end_op();
            return Err(SyscallError::Open);
        }
    }

    // cannot open device out of range
    if inode_inner.r#type == InodeType::Device && inode_inner.major >= NDEV as u16 {
        inode.unlock_put(inode_inner);
        log::end_op();
        return Err(SyscallError::Open);
    }

    // allocate a file structure and a file descriptor
    let (fd, file) = match File::alloc() {
        Ok(mut file) => match fd_alloc(file.clone()) {
            Ok(fd) => (fd, file),
            Err(e) => {
                // if err here, we must also close the file
                file.close();
                inode.unlock_put(inode_inner);
                log::end_op();
                return Err(e);
            }
        },
        Err(_) => {
            inode.unlock_put(inode_inner);
            log::end_op();
            return Err(SyscallError::Open);
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
    file_inner.readable = (o_mode & File::O_WRONLY) == 0;
    file_inner.writeable = (o_mode & File::O_WRONLY) != 0 || (o_mode & File::O_RDWR != 0);

    if (o_mode & File::O_TRUNC) != 0 && inode_inner.r#type == InodeType::File {
        inode.trunc(&mut inode_inner);
    }

    inode.unlock(inode_inner);

    log::end_op();

    Ok(fd)
}

pub fn sys_mkdir(args: &SyscallArgs) -> Result<usize, SyscallError> {
    log::begin_op();

    let path = args
        .fetch_string(args.get_addr(0), MAXPATH)
        .inspect_err(|_| {
            log::end_op();
        })?;

    let (inode, inode_inner) = Inode::create(&Path::new(&path), InodeType::Directory, 0, 0)
        .map_err(|_| {
            log::end_op();
            SyscallError::Mkdir
        })?;

    inode.unlock_put(inode_inner);

    log::end_op();
    Ok(0)
}

pub fn sys_mknod(args: &SyscallArgs) -> Result<usize, SyscallError> {
    log::begin_op();

    let major = args.get_int(1) as u16;
    let minor = args.get_int(2) as u16;
    let path = args
        .fetch_string(args.get_addr(0), MAXPATH)
        .inspect_err(|_| {
            log::end_op();
        })?;

    let (inode, inner) = Inode::create(&Path::new(&path), InodeType::Device, major, minor)
        .map_err(|_| {
            log::end_op();
            SyscallError::Mknod
        })?;

    inode.unlock_put(inner);

    log::end_op();
    Ok(0)
}

pub fn sys_chdir(args: &SyscallArgs) -> Result<usize, SyscallError> {
    let proc = CPU_POOL.current_proc().unwrap();

    log::begin_op();

    let path = args.fetch_string(args.get_addr(0), MAXPATH).map_err(|_| {
        log::end_op();
        SyscallError::Chdir
    })?;

    let inode = Path::new(&path).resolve().map_err(|_| {
        log::end_op();
        SyscallError::Chdir
    })?;

    let inner = inode.lock();

    if inner.r#type != InodeType::Directory {
        inode.unlock_put(inner);
        log::end_op();
        return Err(SyscallError::Chdir);
    }

    inode.unlock(inner);

    let old_cwd = mem::replace(unsafe { &mut proc.data_mut().cwd }, inode);
    old_cwd.put();

    log::end_op();

    Ok(0)
}

pub fn sys_exec(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}

pub fn sys_pipe(_args: &SyscallArgs) -> Result<usize, SyscallError> {
    unimplemented!()
}
