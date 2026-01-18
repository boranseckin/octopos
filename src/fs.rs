use core::ptr;

use crate::buf::BCACHE;
use crate::log;
use crate::sync::OnceLock;

/// Block size
pub const BSIZE: usize = 1024;

pub const FSMAGIC: u32 = 0x10203040;

pub static SB: OnceLock<SuperBlock> = OnceLock::new();

/// On-disk superblock (read at boot)
#[repr(C)]
#[derive(Debug, Clone)]
pub struct SuperBlock {
    /// Must be `FSMAGIC`
    pub magic: u32,
    /// Size of file system image (blocks)
    pub size: u32,
    /// Number of data blocks
    pub nblocks: u32,
    /// Number of inodes
    pub ninodes: u32,
    /// Number of log blocks
    pub nlogs: u32,
    /// Block number of first log block
    pub logstart: u32,
    /// Block number of first inode block
    pub inodestart: u32,
    /// Block number of first free map block
    pub bmapstart: u32,
}

impl SuperBlock {
    /// Reads the superblock from disk and initializes the global `SB`.
    fn initialize(dev: u32) {
        let buf = BCACHE.read(dev, 1); // superblock is at block 1
        let sb = unsafe { ptr::read_unaligned(buf.data().as_ptr() as *const SuperBlock) };
        BCACHE.release(buf);

        assert_eq!(sb.magic, FSMAGIC, "invalid file system");

        SB.initialize(|| Ok::<_, ()>(sb));
    }
}

/// Initialize the file system.
pub fn init(dev: u32) {
    SuperBlock::initialize(dev);
    log::init(dev, SB.get().unwrap());
}
