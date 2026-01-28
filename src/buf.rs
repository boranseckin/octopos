use core::mem::{self, MaybeUninit};

use crate::fs::BSIZE;
use crate::param::NBUF;
use crate::sleeplock::{SleepLock, SleepLockGuard};
use crate::spinlock::SpinLock;
use crate::virtio_disk;

/// Buffer metadata, protected by `BCache`'s `SpinLock`.
#[derive(Debug, Clone)]
pub struct BufMeta {
    pub valid: bool,
    pub disk: bool,
    pub dev: u32,
    pub block_no: u32,
    pub ref_count: u32,
    // LRU linked list using indices
    pub prev: usize,
    pub next: usize,
}

impl BufMeta {
    const fn new() -> Self {
        Self {
            valid: false,
            disk: false,
            dev: 0,
            block_no: 0,
            ref_count: 0,
            prev: 0,
            next: 0,
        }
    }
}

/// Buffer data, protected by `SleepLock` during I/O
#[derive(Debug, Clone)]
pub struct BufData {
    pub data: [u8; BSIZE],
}

impl BufData {
    const fn new() -> Self {
        Self { data: [0; BSIZE] }
    }
}

/// A buffer handle returned by `get()`/`read()`.
/// Holds the `SleepLock` guard for the buffer data.
#[derive(Debug)]
pub struct Buf<'a> {
    pub id: usize,
    pub guard: SleepLockGuard<'a, BufData>,
}

impl Buf<'_> {
    pub fn data(&self) -> &[u8; BSIZE] {
        &self.guard.data
    }

    pub fn data_mut(&mut self) -> &mut [u8; BSIZE] {
        &mut self.guard.data
    }
}

/// Meta data of the buffer cache, protected by `SpinLock`.
#[derive(Debug)]
pub struct BCacheInner {
    pub meta: [BufMeta; NBUF],
    pub head: usize,
}

pub static BCACHE: BCache = BCache::new();

/// Buffer cache.
///
/// The buffer cache is a linked list of buf structures holding cached copies of disk block
/// contents. Caching disk blocks in memory reduces the number of disk reads and also provides a
/// synchronization point for disk blocks used by multiple processes.
///
/// Interface:
/// * To get a buffer for a particular disk block, call `read()`.
/// * After changing buffer data, call `write()` to write it to disk.
/// * When done with the buffer, call `release()`.
/// * Do not use the buffer after calling `release()`.
/// * Only one process at a time can use a buffer, so do not keep them longer than necessary.
#[derive(Debug)]
pub struct BCache {
    /// `SpinLock` protects metadata lookups and LRU manipulations.
    pub inner: SpinLock<BCacheInner>,
    /// Each buffer's data is protected by its own `SleepLock`.
    pub bufs: [SleepLock<BufData>; NBUF],
}

impl BCache {
    const fn new() -> Self {
        let bufs = {
            let mut array: [MaybeUninit<SleepLock<BufData>>; NBUF] =
                unsafe { MaybeUninit::uninit().assume_init() };

            let mut i = 0;
            while i < NBUF {
                array[i] = MaybeUninit::new(SleepLock::new(BufData::new(), "buffer"));
                i += 1;
            }

            unsafe { mem::transmute(array) }
        };

        let meta = {
            let mut array: [MaybeUninit<BufMeta>; NBUF] =
                unsafe { MaybeUninit::uninit().assume_init() };

            let mut i = 0;
            while i < NBUF {
                array[i] = MaybeUninit::new(BufMeta::new());
                i += 1;
            }

            unsafe { mem::transmute(array) }
        };

        Self {
            inner: SpinLock::new(BCacheInner { meta, head: 0 }, "bcache"),
            bufs,
        }
    }

    /// Looks through buffer cache for block on device `dev`.
    /// If not found, allocates a buffer.
    /// Returns buffer's index and locked guard.
    pub fn get(&self, dev: u32, block_no: u32) -> Buf<'_> {
        let mut inner = self.inner.lock();

        // is the block already cached?
        for i in 0..NBUF {
            let meta = &mut inner.meta[i];
            if meta.dev == dev && meta.block_no == block_no {
                meta.ref_count += 1;
                drop(inner);

                let guard = self.bufs[i].lock();
                return Buf { id: i, guard };
            }
        }

        // not cached
        // recycle the least recently used (LRU) unused buffer
        // start from the tail
        let mut i = inner.meta[inner.head].prev;
        loop {
            if i == inner.head {
                panic!("bcache get no buffers");
            }

            let meta = &mut inner.meta[i];
            if meta.ref_count == 0 {
                meta.dev = dev;
                meta.block_no = block_no;
                meta.valid = false;
                meta.ref_count += 1;
                drop(inner);

                let guard = self.bufs[i].lock();
                return Buf { id: i, guard };
            }

            i = inner.meta[i].prev;
        }
    }

    /// Returns a locked buf with the contents of the indicated block.
    pub fn read(&self, dev: u32, block_no: u32) -> Buf<'_> {
        let mut buf = self.get(dev, block_no);

        let valid = {
            let lock = self.inner.lock();
            lock.meta[buf.id].valid
        };

        if !valid {
            virtio_disk::rw(&mut buf, false); // read from disk

            let mut lock = self.inner.lock();
            lock.meta[buf.id].valid = true;
        }

        buf
    }

    /// Writes `buf`'s contents to disk.
    pub fn write(&self, buf: &mut Buf<'_>) {
        // buf must be locked since it holds the sleep lock guard
        virtio_disk::rw(buf, true);
    }

    /// Releases a locked buffer.
    /// Moves to the head of the most-recently-used list.
    ///
    // TODO: possibly handle this with Drop
    pub fn release(&self, buf: Buf<'_>) {
        // buf must be locked since it holds the sleep lock guard

        let id = buf.id;
        drop(buf);

        let mut inner = self.inner.lock();

        inner.meta[id].ref_count -= 1;
        if inner.meta[id].ref_count == 0 {
            // no one is waiting for it

            // from: prev -> current -> next
            // to:   prev -> next
            let next = inner.meta[id].next;
            let prev = inner.meta[id].prev;
            inner.meta[next].prev = inner.meta[id].prev;
            inner.meta[prev].next = inner.meta[id].next;

            // from: head -> first
            // to:   head -> current -> first
            let head = inner.head;
            let first = inner.meta[head].next;
            inner.meta[id].next = first;
            inner.meta[id].prev = head;
            inner.meta[first].prev = id;
            inner.meta[head].next = id;
        }
    }

    /// Artificially increments the reference count for the buffer so that it is not recycled.
    pub fn pin(&self, buf: &Buf<'_>) {
        let mut inner = self.inner.lock();
        inner.meta[buf.id].ref_count += 1;
    }

    /// Artificially decrements the reference count for the buffer.
    pub fn unpin(&self, buf: &Buf<'_>) {
        let mut inner = self.inner.lock();
        inner.meta[buf.id].ref_count -= 1;
    }
}

/// Initialize the buffer cache.
///
/// # Safety
/// This function must be called only once during kernel initialization.
pub unsafe fn init() {
    let mut inner = BCACHE.inner.lock();

    // create a circular doubly-linked list
    // head -> 0 -> 1 -> ... -> NBUF - 1 -> head
    inner.head = 0;
    for i in 0..NBUF {
        inner.meta[i].prev = if i == 0 { NBUF - 1 } else { i - 1 };
        inner.meta[i].next = if i == NBUF - 1 { 0 } else { i + 1 };
    }

    println!("buf  init");
}
