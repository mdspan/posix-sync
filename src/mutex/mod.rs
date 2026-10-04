//! Rust wrappers around POSIX mutexes.
//!
//! This module exposes two kinds of mutexes: [`OwnedMutex`] and [`BorrowedMutex`].
//!
//! |                                    | **`OwnedMutex`** | **`BorrowedMutex`** |
//! |------------------------------------|------------------|---------------------|
//! | Implements `Copy`?                 | No               | Yes                 |
//! | Destroys underlying mutex on drop? | When unlocked    | Never               |
//! | Can be used for shared memory IPC? | No               | Yes                 |
//! | Can be used without `unsafe`?      | Yes              | No                  |
//!
//! The mutexes in this module don't poison, which makes them `!UnwindSafe` and `!RefUnwindSafe`.
//! Shared memory IPC additionally needs the mutex to be built with
//! `with_sharing(MutexSharing::Shared)`, an option not every platform implements: see
//! [Platform support](#platform-support).
//!
//! Mutexes are generic over a [robustness marker](robustness_markers), which corresponds to
//! the POSIX robustness attribute (see
//! [`pthread_mutexattr_setrobust`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_mutexattr_setrobust.html))
//! and specifies the behavior of the mutex when an owner terminates without unlocking it.
//! The marker `Standard` means no special action is taken; that is, the mutex stays locked, so
//! `lock` blocks forever. The marker `Robust` hands the next owner an indeterminate
//! [guard](guards) instead, giving the new owner a single opportunity to repair any state the
//! dead owner may have left invalid (for example, a shared queue whose length was incremented
//! before the new element was written).
//!
//! Mutexes are constructed with [`MutexBuilder`]:
//!
//! ```
//! use posix_sync::mutex::{MutexBuilder, MutexType, robustness_markers::Standard};
//!
//! let mtx = MutexBuilder::<Standard>::new()
//!     .with_type(MutexType::ErrorCheck)
//!     .build_owned();
//!
//! let guard = mtx.lock()?;
//! // ... critical section ...
//! drop(guard);
//! # Ok::<(), posix_sync::mutex::MutexLockError>(())
//! ```
//!
//! The following example puts a robust, process-shared mutex at some memory mapped region with
//! [`build_borrowed`](MutexBuilder::build_borrowed). We use the
//! [`memmap2`](https://crates.io/crates/memmap2) crate to create the mapping, though any
//! RAII mmap wrapper would work.
//!
//! ```
//! # #[cfg(any(target_os = "linux", target_os = "freebsd"))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use posix_sync::mutex::{
//!     MutexBuilder, MutexSharing, RawMutexAlloc,
//!     guards::RobustGuardContainer,
//!     robustness_markers::Robust,
//! };
//!
//! // A shared, page-aligned mapping that another process could map as well.
//! let file = tempfile::tempfile()?;
//! file.set_len(RawMutexAlloc::SIZE as u64)?;
//! let map = memmap2::MmapRaw::map_raw(&file)?;
//!
//! let mtx = unsafe {
//!     MutexBuilder::<Robust>::new()
//!         .with_sharing(MutexSharing::Shared)
//!         .build_borrowed(map.as_mut_ptr() as *mut RawMutexAlloc, &map)
//! };
//!
//! match unsafe { mtx.lock()? } {
//!     RobustGuardContainer::Standard(_guard) => {
//!         // ... critical section ...
//!     }
//!     RobustGuardContainer::Indeterminate(guard) => {
//!         // The last owner died holding the lock. If the protected data is intact, or can be
//!         // repaired here, mark it consistent. Dropping the guard without doing so leaves the
//!         // mutex unrecoverable, and every later lock fails with NotRecoverable.
//!         let _guard = guard.make_consistent()?;
//!     }
//! }
//!
//! unsafe { mtx.destroy() };
//! # Ok(())
//! # }
//! # #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
//! # fn main() {}
//! ```
//!
//! If the mutex was initialized by another process or thread, use [`BorrowedMutex::from_raw`]
//! instead of the builder:
//!
//! ```no_run
//! use posix_sync::mutex::{BorrowedMutex, RawMutexAlloc, robustness_markers::Standard};
//!
//! // A mapping of the file in which another process already initialized a mutex.
//! let file = std::fs::OpenOptions::new()
//!     .read(true)
//!     .write(true)
//!     .open("mutex.shared")?;
//! let map = memmap2::MmapRaw::map_raw(&file)?;
//!
//! // The marker has to match the robustness the initializing process picked; assume it picked
//! // Standard.
//! let mtx: BorrowedMutex<Standard> = unsafe {
//!     BorrowedMutex::from_raw(map.as_mut_ptr() as *mut RawMutexAlloc, &map)
//! };
//!
//! drop(unsafe { mtx.lock()? });
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Platform support
//!
//! See the table at the [crate root](crate#platform-support).

use std::marker::PhantomPinned;
use std::mem::{align_of, size_of};

use libc::pthread_mutex_t;

use crate::utils::AsRawUnderlying;

mod errors;
pub use errors::*;

mod owned;
pub use owned::*;

mod borrowed;
pub use borrowed::*;

pub mod guards;

pub mod robustness_markers;

pub mod builders;
pub use builders::{MutexBuilder, MutexProtocol, MutexType};

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
pub use builders::MutexSharing;

/// A `pthread_mutex_t` that has been made `!Unpin`.
/// Cast to a pointer of this type when constructing a mutex from a raw pointer.
#[repr(transparent)]
pub struct RawMutexAlloc {
    #[allow(dead_code)]
    raw: pthread_mutex_t,
    _phantom: PhantomPinned,
}

impl RawMutexAlloc {
    /// The size, in bytes, of the underlying `pthread_mutex_t`.
    pub const SIZE: usize = size_of::<pthread_mutex_t>();

    /// The alignment a `*mut RawMutexAlloc` must satisfy.
    pub const ALIGN: usize = align_of::<pthread_mutex_t>();
}
