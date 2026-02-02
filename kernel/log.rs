// Simple logging that allows concurrent FS system calls.
//
// A log transaction contains the updates of multiple FS system calls. The logging system only
// commits when there are no FS system calls active. Thus there is never any reasoning required
// about whether a commit might write an uncommitted system call's updates to disk.
//
// A system call should call begin_op()/end_op() to mark its start and end. Usually begin_op() just
// increments the count of in-progress FS system calls and returns. But if it thinks the log is
// close to running out, it sleeps until the last outstanding end_op() commits.
//
// The log is a physical re-do log containing disk blocks.
// The on-disk log format:
//   header block, containing block #s for block A, B, C, ...
//   block A
//   block B
//   block C
//   ...
// Log appends are synchronous.

use crate::buf::{BCACHE, Buf};
use crate::fs::{BSIZE, SuperBlock};
use crate::param::{LOGSIZE, MAXOPBLOCKS};
use crate::proc::{self, Channel};
use crate::spinlock::SpinLock;

/// Contents of the header block, used for both the on-disk header block and to keep track in memory
/// of logged block# before commit.
#[repr(C)]
#[derive(Debug)]
pub struct LogHeader {
    n: u32,
    blocks: [u32; LOGSIZE],
}

#[derive(Debug)]
pub struct LogInner {
    start: u32,
    size: u32,
    outstanding: u32,
    committing: bool,
    dev: u32,
    header: LogHeader,
}

pub static LOG: Log = Log::new();

#[derive(Debug)]
pub struct Log {
    inner: SpinLock<LogInner>,
}

impl Log {
    const fn new() -> Self {
        Self {
            inner: SpinLock::new(
                LogInner {
                    start: 0,
                    size: 0,
                    outstanding: 0,
                    committing: false,
                    dev: 0,
                    header: LogHeader {
                        n: 0,
                        blocks: [0; LOGSIZE],
                    },
                },
                "log",
            ),
        }
    }

    /// Copies committed blocks from log to their home location
    fn install_trans(recovering: bool) {
        let (dev, start, n) = {
            let inner = LOG.inner.lock();
            (inner.dev, inner.start, inner.header.n)
        }; // LOG lock dropped here

        for tail in 0..n {
            let block = {
                let inner = LOG.inner.lock();
                inner.header.blocks[tail as usize]
            }; // LOG lock dropped here

            // read log block
            let lbuf = BCACHE.read(dev, start + tail + 1);
            // read dst
            let mut dbuf = BCACHE.read(dev, block);

            // copy block to dst
            dbuf.data_mut().copy_from_slice(lbuf.data());

            // write dst to disk
            BCACHE.write(&mut dbuf);

            if !recovering {
                BCACHE.unpin(&dbuf);
            }

            BCACHE.release(lbuf);
            BCACHE.release(dbuf);
        }
    }

    /// Reads the log header from disk into the in-memory log header
    ///
    /// # Safety
    /// This function performs raw pointer dereferencing. Make sure `start` is pointing to the
    /// location of the `header`.
    unsafe fn read_head() {
        let (dev, start) = {
            let inner = LOG.inner.lock();
            (inner.dev, inner.start)
        }; // LOG lock dropped here

        let buf = BCACHE.read(dev, start);
        let header = unsafe { &*(buf.data().as_ptr() as *const LogHeader) };

        {
            let mut inner = LOG.inner.lock();
            inner.header.n = header.n;
            for i in 0..inner.header.n {
                inner.header.blocks[i as usize] = header.blocks[i as usize];
            }
        } // LOG lock dropped here

        BCACHE.release(buf);
    }

    /// Writes in-memory log header to disk.
    /// This is the true point at which the current transaction commits.
    ///
    /// # Safety
    /// This function performs raw pointer dereferencing. Make sure `start` is pointing to the
    /// location of the `header`.
    unsafe fn write_head() {
        let (dev, start) = {
            let inner = LOG.inner.lock();
            (inner.dev, inner.start)
        }; // LOG lock dropped here

        let mut buf = BCACHE.read(dev, start);
        let header = unsafe { &mut *(buf.data_mut().as_mut_ptr() as *mut LogHeader) };

        {
            let inner = LOG.inner.lock();
            header.n = inner.header.n;
            for i in 0..inner.header.n {
                header.blocks[i as usize] = inner.header.blocks[i as usize];
            }
        } // LOG lock dropped here

        BCACHE.write(&mut buf);
        BCACHE.release(buf);
    }

    /// Copies modified blocks from cache to log
    fn write_log() {
        let (dev, start, n) = {
            let inner = LOG.inner.lock();
            (inner.dev, inner.start, inner.header.n)
        }; // LOG lock dropped here

        for tail in 0..n {
            let block = {
                let inner = LOG.inner.lock();
                inner.header.blocks[tail as usize]
            }; // LOG lock dropped here

            let mut to = BCACHE.read(dev, start + tail + 1); // log block
            let from = BCACHE.read(dev, block); // cache block

            to.data_mut().copy_from_slice(from.data());
            BCACHE.write(&mut to);

            BCACHE.release(to);
            BCACHE.release(from);
        }
    }
}

/// A guard that begins a log operation on creation and ends it on drop.
/// If the operation did not complete successfully, the optional `on_err` callback is invoked.
#[derive(Debug)]
pub struct Operation<F: FnOnce() = fn()> {
    on_err: Option<F>,
    success: bool,
}

impl Operation {
    pub fn begin() -> Self {
        begin_op();
        Self {
            on_err: None,
            success: false,
        }
    }
}

impl<F: FnOnce()> Operation<F> {
    pub fn begin_with(on_err: F) -> Self {
        begin_op();
        Self {
            on_err: Some(on_err),
            success: false,
        }
    }

    pub fn success(&mut self) {
        self.success = true;
    }
}

impl<F: FnOnce()> Drop for Operation<F> {
    fn drop(&mut self) {
        if !self.success
            && let Some(f) = self.on_err.take()
        {
            f();
        }
        end_op();
    }
}

/// Begins a new operation on the log.
/// Must be called at the start of each FS system call.
fn begin_op() {
    let mut inner = LOG.inner.lock();

    loop {
        if inner.committing {
            inner = proc::sleep(Channel::Log, inner);
        } else if inner.header.n as usize + (inner.outstanding as usize + 1) * MAXOPBLOCKS > LOGSIZE
        {
            // this op might exhaust log space; wait for commit
            inner = proc::sleep(Channel::Log, inner);
        } else {
            inner.outstanding += 1;
            break;
        }
    }
}

/// Ends the current operation on the log.
/// Must be called at the end of each FS system call.
/// Commits if this was the last outstanding operation.
fn end_op() {
    let mut do_commit = false;

    {
        let mut inner = LOG.inner.lock();

        inner.outstanding -= 1;

        if inner.committing {
            panic!("log committing");
        }

        if inner.outstanding == 0 {
            do_commit = true;
            inner.committing = true;
        } else {
            // `begin_op()` may be waiting for log space, and decrementing `outstanding` has
            // decreased the amount of reserved space
            proc::wakeup(Channel::Log);
        }
    } // drop inner lock

    if do_commit {
        // call commit without holding locks, since not allowed to sleep with locks
        commit();
        let mut inner = LOG.inner.lock();
        inner.committing = false;
        proc::wakeup(Channel::Log);
    }
}

/// Commits the current transaction.
fn commit() {
    let n = {
        let inner = LOG.inner.lock();
        inner.header.n
    };

    if n > 0 {
        // write modified blocks from cache to log
        Log::write_log();
        // write header to disk -- the real commit
        unsafe { Log::write_head() };
        // now install write to home location
        Log::install_trans(false);

        {
            let mut inner = LOG.inner.lock();
            inner.header.n = 0;
        }

        // erase the transactions from the log
        unsafe { Log::write_head() };
    }
}

/// Caller has modified `buf` and is done with the buffer.
/// Record the block number and pin in the cache by increasing ref count.
/// `commit()`/`write_log()` will do the disk write.
///
/// `write()` replaces `BCACHE::write()`
pub fn write(buf: &Buf<'_>) {
    let mut inner = LOG.inner.lock();

    if inner.header.n as usize >= LOGSIZE || inner.header.n >= inner.size - 1 {
        panic!("log_write: transaction too big");
    }

    if inner.outstanding < 1 {
        panic!("log_write: outside of trans");
    }

    let block_no = {
        let bcache = BCACHE.inner.lock();
        bcache.meta[buf.id].block_no
    };

    let mut i = 0;
    while i < inner.header.n as usize {
        if inner.header.blocks[i] == block_no {
            // log absorption
            break;
        }

        i += 1;
    }

    inner.header.blocks[i] = block_no;

    if i == inner.header.n as usize {
        BCACHE.pin(buf);
        inner.header.n += 1;
    }
}

/// Recovers the log by installing any committed transactions found in the log on disk.
///
/// # Safety
/// This should only be called at file system initialization and after `Log` init.
pub unsafe fn recover_from_log() {
    unsafe { Log::read_head() };

    // if committed, copy from log to disk
    Log::install_trans(true);

    // clear the log
    {
        let mut inner = LOG.inner.lock();
        inner.header.n = 0;
    }
    unsafe { Log::write_head() };
}

/// Initialize the log system.
pub fn init(dev: u32, sb: &SuperBlock) {
    if size_of::<LogHeader>() >= BSIZE {
        panic!("init_log: log header too big");
    }

    {
        let mut inner = LOG.inner.lock();
        inner.start = sb.logstart;
        inner.size = sb.nlogs;
        inner.dev = dev;
    }

    // # Safety: This is called after log initialization.
    unsafe { recover_from_log() };
}
