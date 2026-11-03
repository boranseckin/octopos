//! The names nearly every program uses. Start a program with `use user::prelude::*;`
//! and import anything else from its module.

pub use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

pub use crate::env::Args;
pub use crate::io::{Fd, Read, Write};
pub use crate::process::{exit, exit_with_msg};
pub use crate::{eprint, eprintln, print, println};

pub use kernel::abi::{Ipv4Addr, OpenFlag, SockAddr, SysError};
