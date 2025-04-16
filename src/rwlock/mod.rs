//! Rust wrappers around POSIX reader/writer locks.
//!
//! This module exposes two kinds of rwlocks: [`OwnedRwLock`] and [`BorrowedRwLock`].
//! Both wrap an underlying `libc::pthread_rwlock_t`, the reader/writer lock object POSIX defines,
//! but differ in the following ways:
//!
//! |                                     | **`OwnedRwLock`** | **`BorrowedRwLock`** |
//! |-------------------------------------|-------------------|----------------------|
//! | Implements `Copy`?                  | No                | Yes                  |
//! | Destroys underlying rwlock on drop? | When unlocked     | Never                |
//! | Can be used for shared memory IPC?  | No                | Yes                  |
//! | Can be used without `unsafe`?       | Yes               | No                   |
//!
//! Neither poisons. Shared memory IPC
//! additionally needs the rwlock to be built with `with_sharing(RwLockSharing::Shared)`, an
//! option not every platform implements: see [Platform support](#platform-support).
//!
//! Unlike mutexes, there is no robustness attribute for rwlocks in POSIX, meaning that
//! if a reader/writer dies before releasing the lock, the rwlock will be stuck in
//! that state permanently.
//!
//! Beyond sharing, a rwlock has a preference: who to prefer when readers and writers contend. POSIX
//! only defines this for threads running under the realtime scheduling policies; everywhere else it
//! leaves the outcome to the implementation. glibc fills the gap with an extension of its own,
//! `pthread_rwlockattr_setkind_np`, which [`RwLockBuilder`] exposes as `with_preference` on
//! glibc alone.
//!
//! glibc's default prefers readers: one can take the lock even while a writer is waiting, so a
//! steady stream of readers can starve a writer indefinitely. A writer-preferring lock instead
//! blocks readers that arrive while a writer waits, which stops the starvation at the price of
//! letting recursive read locking deadlock; `RwLockPreference` documents the three choices.
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
//! The next example puts a process-shared rwlock and the value it guards into one memory mapped
//! region with [`build_borrowed`](RwLockBuilder::build_borrowed), reserving
//! [`RawRwLockAlloc::SIZE`] bytes for the lock at an offset aligned to [`RawRwLockAlloc::ALIGN`].
//! The writer and the readers would normally be separate processes mapping the same file; here a
//! single thread stands in for all of them, making the same calls they would.
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
//! If the rwlock was initialised by another process, use [`BorrowedRwLock::from_raw`] instead of
//! the builder:
//!
//! ```no_run
//! use posix_sync::rwlock::{BorrowedRwLock, RawRwLockAlloc};
//!
//! // A mapping of the file in which another process already initialised a rwlock.
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
//! Process sharing, timed locking and the reader/writer preference are not available everywhere.
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
/// This is a `pthread_rwlock_t` that has been made `!Unpin`, because a rwlock must not be
/// relocated once it has been initialised. The two associated constants describe how much room to
/// leave for one when carving up a shared mapping by hand.
#[repr(transparent)]
pub struct RawRwLockAlloc {
    // the storage itself. it is only ever touched through a `*mut pthread_rwlock_t`.
    #[allow(dead_code)]
    raw: pthread_rwlock_t,

    /// This field is here because [`pthread_rwlock_t`] is `Unpin`.
    _phantom: PhantomPinned,
}

impl RawRwLockAlloc {
    /// The size, in bytes, of the underlying `pthread_rwlock_t`.
    pub const SIZE: usize = size_of::<pthread_rwlock_t>();

    /// The alignment a `*mut RawRwLockAlloc` must satisfy.
    pub const ALIGN: usize = align_of::<pthread_rwlock_t>();
}
