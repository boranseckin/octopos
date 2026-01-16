use crate::fs::BSIZE;
use crate::spinlock::SpinLock;

/// Number of buffers in the buffer cache.
const NBUF: usize = 30;

/// In-memory buffer for disk block.
#[derive(Debug, Clone)]
pub struct Buf {
    pub valid: bool,
    pub disk: bool,
    pub dev: usize,
    pub block_no: usize,
    pub ref_count: usize,

    // LRU linked list using indices
    pub prev: usize,
    pub next: usize,

    pub data: [u8; BSIZE],
}

pub static BCACHE: SpinLock<BCache> = SpinLock::new(BCache::new(), "bcache");

/// Buffer cache.
///
/// The buffer cache is a linked list of buf structures holding cached copies of disk block
/// contents. Caching disk blocks in memory reduces the number of disk reads and also provides a
/// synchronization point for disk blocks used by multiple processes.
///
/// Interface:
/// * To get a buffer for a particular disk block, call bread.
/// * After changing buffer data, call bwrite to write it to disk.
/// * When done with the buffer, call brelse.
/// * Do not use the buffer after calling brelse.
/// * Only one process at a time can use a buffer, so do not keep them longer than necessary.
#[derive(Debug)]
pub struct BCache {
    buf: [Buf; NBUF],

    /// Head of LRU list (index)
    /// head.next is most recent, head.prev is least
    head: usize,
}

impl BCache {
    const fn new() -> Self {
        unimplemented!()
    }

    /// Moves the buffer with the given id to the front of the LRU list.
    fn move_to_front(&mut self, id: usize) {
        // Remove from current position
        let prev = self.buf[id].prev;
        let next = self.buf[id].next;
        self.buf[prev].next = next;
        self.buf[next].prev = prev;

        // Insert at front
        let first = self.buf[self.head].next;
        self.buf[id].next = first;
        self.buf[id].prev = self.head;
        self.buf[self.head].next = id;
        self.buf[first].prev = id;
    }

    /// Finds the least recently used buffer with ref_count == 0.
    fn find_lru(&self) -> Option<usize> {
        let mut current = self.buf[self.head].prev; // Start from the end of the list
        while current != self.head {
            if self.buf[current].ref_count == 0 {
                return Some(current);
            }
            current = self.buf[current].prev;
        }
        None
    }
}
