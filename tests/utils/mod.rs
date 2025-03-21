//! Shared scaffolding for the integration tests: a memory mapping to put shared primitives in,
//! and enough process plumbing to watch a child die while holding a lock.

#![allow(dead_code)]

use std::cmp::PartialOrd;
use std::ffi::c_int;
use std::io;
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use libc::pid_t;

pub use anyhow::{anyhow, Result};

/// Wrapper around a memory mapping that calls munmap when dropped.
pub struct MmapRAII {
    ptr: *mut u8,
    size: usize,
}

impl MmapRAII {
    /// Creates an anonymous, shared mapping with at least `size` bytes.
    pub fn new(size: usize) -> Self {
        let ptr = unsafe {
            let ptr = libc::mmap(
                ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANONYMOUS,
                -1,
                0,
            );
            assert_ne!(ptr, libc::MAP_FAILED);
            ptr as *mut _
        };
        MmapRAII { ptr, size }
    }

    /// Returns a pointer to the start of the memory mapping.
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr as *const _
    }

    /// Returns a pointer to the start of the memory mapping. It takes `&self` because the whole
    /// point of the mapping is that it is written through raw pointers, potentially by another
    /// process, so a `&mut` would be claiming an exclusivity that does not exist.
    pub fn as_mut_ptr(&self) -> *mut u8 {
        self.ptr
    }

    /// Returns a pointer `offset` bytes into the mapping.
    ///
    /// # Safety
    /// `offset` must be within the mapping.
    pub unsafe fn offset_ptr(&self, offset: usize) -> *mut u8 {
        debug_assert!(offset < self.size);
        self.ptr.add(offset)
    }

    /// Returns the passed-in size of the memory mapping.
    pub fn size(&self) -> usize {
        self.size
    }
}

impl Drop for MmapRAII {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.ptr as *mut _, self.size);
        }
    }
}

/// Rounds `offset` up to the next multiple of `align`, for laying several primitives out inside a
/// single mapping by hand.
pub fn align_up(offset: usize, align: usize) -> usize {
    (offset + align - 1) & !(align - 1)
}

#[allow(clippy::missing_safety_doc)]
pub unsafe fn fork() -> io::Result<c_int> {
    err_if_negative(libc::fork())
}

/// Forks and runs `f` in the child, which exits rather than returning: a child that fell back
/// into the test harness would go on to run the rest of the test binary as well.
///
/// The child exits with 0 if `f` succeeded and 1 otherwise, which is exactly what
/// [`wait_for_process`] looks for.
///
/// It leaves through `_exit` rather than `process::exit`. fork() only carries the calling thread
/// into the child, so running libc's exit handlers there means flushing stdio behind locks that
/// one of the threads left behind may have been holding at the moment of the fork, and the child
/// hangs. The same reasoning is why the failure path writes to fd 2 by hand instead of using
/// `eprintln!`.
#[allow(clippy::missing_safety_doc)]
pub unsafe fn in_child<F>(f: F) -> Result<pid_t>
where
    F: FnOnce() -> Result<()>,
{
    let pid = fork()?;
    if pid != 0 {
        return Ok(pid);
    }

    let code = match f() {
        Ok(()) => 0,
        Err(e) => {
            let message = format!("child failed: {e}\n");
            libc::write(2, message.as_ptr() as *const _, message.len());
            1
        }
    };
    libc::_exit(code);
}

/// Waits for a process to exit successfully within the given timeout, or reports an error. If the
/// timeout is not specified, uses a default of 2 seconds.
///
/// This polls `waitpid` rather than blocking on a pidfd, because pidfds are a Linux facility and
/// the rest of this crate runs on the BSDs and Darwin too.
#[allow(clippy::missing_safety_doc)]
pub unsafe fn wait_for_process(pid: pid_t, timeout: Option<Duration>) -> Result<()> {
    const DEFAULT_TIMEOUT: Duration = Duration::from_millis(2000);
    const POLL_INTERVAL: Duration = Duration::from_millis(2);

    let deadline = Instant::now() + timeout.unwrap_or(DEFAULT_TIMEOUT);
    let mut status: c_int = 0;

    loop {
        match err_if_negative(libc::waitpid(pid, ptr::addr_of_mut!(status), libc::WNOHANG))? {
            0 => {}
            _ => return exit_status(status),
        }

        if Instant::now() >= deadline {
            libc::kill(pid, libc::SIGKILL);
            let _ = libc::waitpid(pid, ptr::addr_of_mut!(status), 0);
            return Err(anyhow!(
                "Timed out while waiting for child process to exit."
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Decodes what `waitpid` filled in, insisting on a clean exit with code 0.
fn exit_status(status: c_int) -> Result<()> {
    if !libc::WIFEXITED(status) {
        return Err(anyhow!(
            "Child process did not exit normally (signal {}).",
            libc::WTERMSIG(status)
        ));
    }
    match libc::WEXITSTATUS(status) {
        0 => Ok(()),
        code => Err(anyhow!("Child process exited with code {code}.")),
    }
}

/// Returns the last IO error if the value is negative or the value itself otherwise.
fn err_if_negative<T: Default + PartialOrd>(val: T) -> io::Result<T> {
    if val < T::default() {
        return Err(io::Error::last_os_error());
    }
    Ok(val)
}
