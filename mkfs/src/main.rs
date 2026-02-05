// Disk Layout:
// [ boot block | sb block | log | inode blocks | free bit map | data blocks ]

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::{env::args, fs::OpenOptions};

use bytemuck::{Pod, Zeroable};

/// max # of blocks any FS op writes
const MAXOPBLOCKS: u32 = 10;
/// max data blocks in on-disk log
const LOGBLOCKS: u32 = MAXOPBLOCKS * 3;
/// size of file system in blocks
const FSSIZE: u32 = 2000;

/// File system magic number
const FSMAGIC: u32 = 0x10203040;
/// Root inode number
pub const ROOTINO: u32 = 1;
/// Block size
const BSIZE: u32 = 1024;
/// Number of direct block addresses in inode
const NDIRECT: u32 = 12;
/// Number of indirect block addresses in inode
const NINDIRECT: u32 = BSIZE / (size_of::<u32>() as u32);
/// Max file size (blocks)
const MAXFILE: u32 = NDIRECT + NINDIRECT;
/// Directory entry name size
const DIRSIZE: usize = 14;

/// Inodes per block
const IPB: u32 = BSIZE / (size_of::<DiskInode>() as u32);
/// Bitmap bits per block
const BPB: u32 = BSIZE * 8;

const NINODES: u32 = 200;
const NBITMAP: u32 = FSSIZE / BPB + 1;
const NINODEBLOCKS: u32 = NINODES / IPB + 1;
/// Header followed by LOGBLOCKS data blocks
const NLOG: u32 = LOGBLOCKS + 1;
/// Number of meta blocks (boot, sb, nlog, inode, bitmap)
const NMETA: u32 = NLOG + NINODEBLOCKS + NBITMAP + 2;
/// Number of data blocks
const NBLOCKS: u32 = FSSIZE - NMETA;

/// On-disk superblock (read at boot)
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct SuperBlock {
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

/// Inode types
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct InodeType(u16);

impl InodeType {
    const _FREE: Self = Self(0);
    const DIRECTORY: Self = Self(1);
    const _FILE: Self = Self(2);
    const _DEVICE: Self = Self(3);
}

/// On-disk inode structure
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct DiskInode {
    /// File type
    pub r#type: InodeType,
    /// Major device number
    pub major: u16,
    /// Minor device number
    pub minor: u16,
    /// Number of links to inode in file system
    pub nlink: u16,
    // Size of file (bytes)
    pub size: u32,
    // Data block addresses
    pub addrs: [u32; NDIRECT as usize + 1],
}

impl DiskInode {
    fn new(r#type: InodeType) -> Self {
        Self {
            r#type,
            major: 0,
            minor: 0,
            nlink: 0,
            size: 0,
            addrs: [0; NDIRECT as usize + 1],
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Directory {
    pub inum: u16,
    pub name: [u8; DIRSIZE],
}

fn main() {
    // the first block we can allocate
    let mut free_block = NMETA;
    let mut free_inode = 1;

    let args = args().collect::<Vec<String>>();

    if args.len() < 2 {
        println!("Usage: mkfs <fs.img> [files]");
        std::process::exit(1);
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&args[1])
        .expect("failed to open file");

    let sb = SuperBlock {
        magic: FSMAGIC,
        size: FSSIZE,
        nblocks: NBLOCKS,
        ninodes: NINODES,
        nlogs: NLOG,
        logstart: 2u32,
        inodestart: (2 + NLOG),
        bmapstart: (2 + NLOG + NINODEBLOCKS),
    };

    println!("{:?}", sb);

    const ZEROS: [u8; BSIZE as usize] = [0u8; BSIZE as usize];
    for i in 0..FSSIZE {
        write_sector(&file, i, &ZEROS);
    }

    let mut buf = [0u8; BSIZE as usize];
    buf[..size_of::<SuperBlock>()].copy_from_slice(bytemuck::bytes_of(&sb));
    write_sector(&file, 1, &buf);

    let rootino = allocate_inode(&file, InodeType::DIRECTORY, &mut free_inode);
    assert_eq!(rootino, ROOTINO);

    let mut de = Directory {
        inum: ROOTINO as u16,
        name: [0u8; DIRSIZE],
    };
    de.name[..1].copy_from_slice(b".");
    append_inode(&file, &mut free_block, rootino, bytemuck::bytes_of(&de));

    de.name[..2].copy_from_slice(b"..");
    append_inode(&file, &mut free_block, rootino, bytemuck::bytes_of(&de));

    // fix size of root inode dir
    let mut din = read_inode(&file, rootino);
    din.size = ((din.size / BSIZE) + 1) * BSIZE;
    write_inode(&file, rootino, &din);

    allocate_block(&file, free_block, sb.bmapstart);

    println!("done");
}

fn write_sector(file: &File, sec: u32, buf: &[u8]) {
    file.write_at(buf, (sec * BSIZE) as u64)
        .expect("failed to write sector");
}

fn read_sector(file: &File, sec: u32, buf: &mut [u8]) {
    file.read_at(buf, (sec * BSIZE) as u64)
        .expect("failed to read sector");
}

fn write_inode(file: &File, inum: u32, inode: &DiskInode) {
    let mut buf = [0u8; BSIZE as usize];
    // inode start + (inum number / inode per block)
    let block_num = 2 + NLOG + (inum / IPB);

    read_sector(file, block_num, &mut buf);

    let offset = (inum % IPB) as usize * size_of::<DiskInode>();
    buf[offset..offset + size_of::<DiskInode>()].copy_from_slice(bytemuck::bytes_of(inode));

    write_sector(file, block_num, &buf);
}

fn read_inode(file: &File, inum: u32) -> DiskInode {
    let mut buf = [0u8; BSIZE as usize];
    // inode start + (inum number / inode per block)
    let block_num = 2 + NLOG + (inum / IPB);

    read_sector(file, block_num, &mut buf);

    let offset = (inum % IPB) as usize * size_of::<DiskInode>();
    *bytemuck::from_bytes::<DiskInode>(&buf[offset..offset + size_of::<DiskInode>()])
}

fn allocate_inode(file: &File, r#type: InodeType, free_inode: &mut u32) -> u32 {
    let inum = *free_inode;
    *free_inode += 1;

    let mut din = DiskInode::new(r#type);
    din.nlink = 1u16;
    din.size = 0u32;

    write_inode(file, inum, &din);
    inum
}

fn append_inode(file: &File, free_block: &mut u32, inum: u32, mut data: &[u8]) {
    let mut buf = [0u8; BSIZE as usize];

    let mut din = read_inode(file, inum);
    let mut offset = din.size;

    while !data.is_empty() {
        let fbn = offset / BSIZE;
        assert!(fbn < MAXFILE);

        let x = if fbn < NDIRECT {
            if din.addrs[fbn as usize] == 0 {
                din.addrs[fbn as usize] = *free_block;
                *free_block += 1;
            }
            din.addrs[fbn as usize]
        } else {
            if din.addrs[NDIRECT as usize] == 0 {
                din.addrs[NDIRECT as usize] = *free_block;
                *free_block += 1;
            }

            let mut indirect = [0u8; BSIZE as usize];
            read_sector(file, din.addrs[NDIRECT as usize], &mut indirect);

            let index = (fbn - NDIRECT) as usize;
            let byte_offset = index * 4;
            let bytes: [u8; 4] = indirect[byte_offset..byte_offset + 4].try_into().unwrap();
            let mut block_addr = u32::from_le_bytes(bytes);
            if block_addr == 0 {
                block_addr = *free_block;
                *free_block += 1;
                indirect[byte_offset..byte_offset + 4].copy_from_slice(&block_addr.to_le_bytes());
                write_sector(file, din.addrs[NDIRECT as usize], &indirect);
            }

            block_addr
        };

        let block_offset = (offset - fbn * BSIZE) as usize;
        let n1 = data.len().min(((fbn + 1) * BSIZE - offset) as usize);

        read_sector(file, x, &mut buf);
        buf[block_offset..block_offset + n1].copy_from_slice(&data[..n1]);
        write_sector(file, x, &buf);

        offset += n1 as u32;
        data = &data[n1..];
    }

    din.size = offset;
    write_inode(file, inum, &din);
}

fn allocate_block(file: &File, used: u32, bmapstart: u32) {
    println!("first {used} blocks have been allocated");
    assert!(used < BPB);

    let mut buf = [0u8; BSIZE as usize];

    for i in 0..used as usize {
        buf[i / 8] |= 0x1 << (i % 8);
    }

    println!("write bitmap block at sector {bmapstart}");
    write_sector(file, bmapstart, &buf);
}
