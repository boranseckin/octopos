use kernel::abi::SysError;

use crate::syscall::{check_unit, raw};

pub fn random(buf: &mut [u8]) -> Result<(), SysError> {
    check_unit(raw::random(buf.as_mut_ptr(), buf.len()))
}
