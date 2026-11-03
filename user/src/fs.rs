use kernel::abi::SysError;

use crate::io::Fd;
use crate::syscall::{check, check_unit, raw, validate_path};

pub use kernel::abi::{DIRSIZE, Directory, InodeType, MAXPATH, OpenFlag, Stat};

pub fn open(path: &str, flags: usize) -> Result<Fd, SysError> {
    let cpath = validate_path(path)?;
    check(raw::open(cpath.as_ptr(), flags)).map(Fd)
}

pub fn mknod(path: &str, major: usize, minor: usize) -> Result<(), SysError> {
    let cpath = validate_path(path)?;
    check_unit(raw::mknod(cpath.as_ptr(), major, minor))
}

pub fn link(old: &str, new: &str) -> Result<(), SysError> {
    let cold = validate_path(old)?;
    let cnew = validate_path(new)?;
    check_unit(raw::link(cold.as_ptr(), cnew.as_ptr()))
}

pub fn unlink(path: &str) -> Result<(), SysError> {
    let cpath = validate_path(path)?;
    check_unit(raw::unlink(cpath.as_ptr()))
}

pub fn mkdir(path: &str) -> Result<(), SysError> {
    let cpath = validate_path(path)?;
    check_unit(raw::mkdir(cpath.as_ptr()))
}

pub fn fstat(fd: Fd, stat: &mut Stat) -> Result<(), SysError> {
    check_unit(raw::fstat(fd.as_raw(), stat as *mut Stat))
}

pub fn chdir(path: &str) -> Result<(), SysError> {
    let cpath = validate_path(path)?;
    check_unit(raw::chdir(cpath.as_ptr()))
}
