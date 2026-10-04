//! Rust wrappers around POSIX reader/writer locks.
//!
//! This module exposes two kinds of rwlocks: [`OwnedRwLock`] and [`BorrowedRwLock`].
//!
//! |                                     | **`OwnedRwLock`** | **`BorrowedRwLock`** |
//! |-------------------------------------|-------------------|----------------------|
//! | Implements `Copy`?                  | No                | Yes                  |
//! | Destroys underlying rwlock on drop? | When unlocked     | Never                |
//! | Can be used for shared memory IPC?  | No                | Yes                  |
//! | Can be used without `unsafe`?       | Yes               | No                   |
//!
//! The rwlocks in this module don't poison, which makes them `!UnwindSafe` and `!RefUnwindSafe`.
//! Shared memory IPC additionally needs the rwlock to be built with
//! `with_sharing(RwLockSharing::Shared)`, which not every platform implements: see
//! [Platform support](#platform-support).
//!
//! In contrast with mutexes, the POSIX standard does not define robust rwlocks.
//!
//! On glibc, rwlocks also have a preference, which specifies who gets the lock when a reader
//! requests it while a writer is waiting. POSIX defines this only for threads running under the
//! realtime scheduling policies and leaves every other case to the implementation. The preference
//! is set with `with_preference`, which corresponds to the GNU extension
//! [`pthread_rwlockattr_setkind_np`](https://man7.org/linux/man-pages/man3/pthread_rwlockattr_setkind_np.3.html).
//!
//! The variants of `RwLockPreference` document the behavior of each preference.
//!
//! Rwlocks are constructed with [`RwLockBuilder`]:
//!
//! ```
//! use posix_sync::rwlock::RwLockBuilder;
//!
//! let lock = RwLockBuilder::new().build_owned();
//!
//! let first = lock.read()?;
//! let second = lock.read()?;
//!
//! // Readers do not exclude each other, but they do exclude writers.
//! assert!(lock.try_write()?.is_none());
//!
//! drop(first);
//! drop(second);
//! assert!(lock.try_write()?.is_some());
//! # Ok::<(), posix_sync::rwlock::RwLockError>(())
//! ```
//!
//! The following example puts a process-shared rwlock and the value it guards into one memory
//! mapped region with [`build_borrowed`](RwLockBuilder::build_borrowed), reserving
//! [`RawRwLockAlloc::SIZE`] bytes for the lock at an offset aligned to [`RawRwLockAlloc::ALIGN`].
//! In practice the writer and the readers would be separate processes mapping the same file. Here
//! a single thread makes the same calls.
//!
//! ```
//! # #[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use std::mem::size_of;
//!
//! use posix_sync::rwlock::{RawRwLockAlloc, RwLockBuilder, RwLockSharing};
//!
//! fn align_up(offset: usize, align: usize) -> usize {
//!     (offset + align - 1) & !(align - 1)
//! }
//!
//! // The value first, then the rwlock guarding it, in one shared mapping.
//! let lock_offset = align_up(size_of::<u64>(), RawRwLockAlloc::ALIGN);
//!
//! let file = tempfile::tempfile()?;
//! file.set_len((lock_offset + RawRwLockAlloc::SIZE) as u64)?;
//! let map = memmap2::MmapRaw::map_raw(&file)?;
//!
//! let lock = unsafe {
//!     RwLockBuilder::new()
//!         .with_sharing(RwLockSharing::Shared)
//!         .build_borrowed(map.as_mut_ptr().add(lock_offset) as *mut RawRwLockAlloc, &map)
//! };
//!
//! let value = map.as_mut_ptr() as *mut u64;
//!
//! let guard = unsafe { lock.write()? };
//! unsafe { value.write_volatile(42) };
//! drop(guard);
//!
//! // Another process mapping the same file could hold read locks of its own alongside these.
//! let first = unsafe { lock.read()? };
//! let second = unsafe { lock.read()? };
//! assert_eq!(unsafe { value.read_volatile() }, 42);
//! drop(first);
//! drop(second);
//!
//! unsafe { lock.destroy() };
//! # Ok(())
//! # }
//! # #[cfg(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))]
//! # fn main() {}
//! ```
//!
//! If the rwlock was initialized by another process or thread, use [`BorrowedRwLock::from_raw`]
//! instead of the builder:
//!
//! ```no_run
//! use posix_sync::rwlock::{BorrowedRwLock, RawRwLockAlloc};
//!
//! // A mapping of the file in which another process already initialized a rwlock.
//! let file = std::fs::OpenOptions::new()
//!     .read(true)
//!     .write(true)
//!     .open("rwlock.shared")?;
//! let map = memmap2::MmapRaw::map_raw(&file)?;
//!
//! let lock = unsafe { BorrowedRwLock::from_raw(map.as_mut_ptr() as *mut RawRwLockAlloc, &map) };
//!
//! drop(unsafe { lock.read()? });
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Platform support
//!
//! See the table at the [crate root](crate#platform-support).

use std::marker::PhantomPinned;
use std::mem::{align_of, size_of};

use libc::pthread_rwlock_t;

use crate::utils::AsRawUnderlying;

mod errors;
pub use errors::*;

mod owned;
pub use owned::*;

mod borrowed;
pub use borrowed::*;

pub mod guards;

pub mod builders;
pub use builders::RwLockBuilder;

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
pub use builders::RwLockSharing;

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub use builders::RwLockPreference;

/// Cast to a pointer of this type when constructing a rwlock from a raw pointer.
///
/// This is a `pthread_rwlock_t` that has been made `!Unpin`.
#[repr(transparent)]
pub struct RawRwLockAlloc {
    #[allow(dead_code)]
    raw: pthread_rwlock_t,
    _phantom: PhantomPinned,
}

impl RawRwLockAlloc {
    /// The size, in bytes, of the underlying `pthread_rwlock_t`.
    pub const SIZE: usize = size_of::<pthread_rwlock_t>();

    /// The alignment a `*mut RawRwLockAlloc` must satisfy.
    pub const ALIGN: usize = align_of::<pthread_rwlock_t>();
}
