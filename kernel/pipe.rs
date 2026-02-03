use alloc::sync::Arc;

use crate::file::{FILE_TABLE, File, FileType};
use crate::fs::FsError;
use crate::proc::{self, Channel, current_proc_and_data_mut};
use crate::spinlock::SpinLock;
use crate::syscall::SyscallError;
use crate::vm::VA;

const PIPESIZE: usize = 512;

#[derive(Debug)]
/// Inner state of a pipe
pub struct PipeInner {
    /// Pipe data buffer
    data: [u8; PIPESIZE],
    /// Number of bytes read
    num_read: usize,
    /// Number of bytes written
    num_write: usize,
    /// Read fd is still open
    read_open: bool,
    /// Write fd is still open
    write_open: bool,
}

#[derive(Debug)]
/// Represents a pipe for inter-process communication
pub struct Pipe {
    inner: SpinLock<PipeInner>,
}

impl Pipe {
    /// Allocates a new pipe and returns the read and write file descriptors
    pub fn alloc() -> Result<(File, File), FsError> {
        let mut f0 = try_log!(File::alloc());

        let mut f1 = match log!(File::alloc()) {
            Ok(file) => file,
            Err(err) => {
                f0.close();
                return Err(err);
            }
        };

        // arc allocates pipe on the heap
        let Ok(pipe) = log!(Arc::try_new(Pipe {
            inner: SpinLock::new(
                PipeInner {
                    data: [0; PIPESIZE],
                    num_read: 0,
                    num_write: 0,
                    read_open: true,
                    write_open: true,
                },
                "pipe",
            ),
        })) else {
            f0.close();
            f1.close();
            err!(FsError::OutOfPipe)
        };

        // f0 = read end
        {
            let mut f0_inner = FILE_TABLE.inner[f0.id].lock();
            f0_inner.r#type = FileType::Pipe {
                pipe: Arc::clone(&pipe),
            };
            f0_inner.readable = true;
            f0_inner.writeable = false;
        }

        // f1 = write end
        {
            let mut f1_inner = FILE_TABLE.inner[f1.id].lock();
            f1_inner.r#type = FileType::Pipe { pipe };
            f1_inner.readable = false;
            f1_inner.writeable = true;
        }

        Ok((f0, f1))
    }

    /// Returns the Arc pointer address as pipe id
    /// The pointer will be unique and constant for the life time of this pipe.
    fn pipe_id(&self) -> usize {
        self as *const Pipe as usize
    }

    /// Closes the pipe's read or write end
    pub fn close(&self, writeable: bool) {
        let mut inner = self.inner.lock();

        if writeable {
            inner.write_open = false;
            proc::wakeup(Channel::PipeRead(self.pipe_id()));
        } else {
            inner.read_open = false;
            proc::wakeup(Channel::PipeWrite(self.pipe_id()));
        }

        // If both ends close, Arc handles deallocation on drop.
    }

    /// Writes to the pipe from the user space
    pub fn write(&self, addr: VA, n: usize) -> Result<usize, SyscallError> {
        let (proc, data) = current_proc_and_data_mut();

        let mut inner = self.inner.lock();

        let mut i = 0;
        while i < n {
            if !inner.read_open || proc.is_killed() {
                err!(SyscallError::Write);
            }

            if inner.num_write == inner.num_read + PIPESIZE {
                proc::wakeup(Channel::PipeRead(self.pipe_id()));
                inner = proc::sleep(Channel::PipeWrite(self.pipe_id()), inner);
            } else {
                let mut ch = [0u8];
                if log!(data.pagetable_mut().copy_from(addr + i, &mut ch)).is_err() {
                    break;
                }

                let index = inner.num_write % PIPESIZE;
                inner.data[index] = ch[0];
                inner.num_write += 1;
                i += 1;
            }
        }

        proc::wakeup(Channel::PipeRead(self.pipe_id()));

        Ok(i)
    }

    /// Reads from the pipe into the user space
    pub fn read(&self, addr: VA, n: usize) -> Result<usize, SyscallError> {
        let (proc, data) = current_proc_and_data_mut();

        let mut inner = self.inner.lock();

        let mut i = 0;

        while inner.num_read == inner.num_write && inner.write_open {
            if proc.is_killed() {
                err!(SyscallError::Read);
            }

            inner = proc::sleep(Channel::PipeRead(self.pipe_id()), inner);
        }

        while i < n {
            if inner.num_read == inner.num_write {
                break;
            }

            let ch = inner.data[inner.num_read % PIPESIZE];
            if log!(data.pagetable_mut().copy_to(&[ch], addr + i)).is_err() {
                err!(SyscallError::Read);
            }

            inner.num_read += 1;
            i += 1;
        }

        proc::wakeup(Channel::PipeWrite(self.pipe_id()));

        Ok(i)
    }
}
