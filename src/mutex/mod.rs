//! Rust wrappers around POSIX mutexes.
//!
//! This module exposes two kinds of mutexes: [`OwnedMutex`] and [`BorrowedMutex`].
//! Both wrap an underlying `libc::pthread_mutex_t`, the mutex object POSIX defines, but differ
//! in the following ways:
//!
//! |                                    | **`OwnedMutex`** | **`BorrowedMutex`** |
//! |------------------------------------|------------------|---------------------|
//! | Implements `Copy`?                 | No               | Yes                 |
//! | Destroys underlying mutex on drop? | When unlocked    | Never               |
//! | Can be used for shared memory IPC? | No               | Yes                 |
//! | Can be used without `unsafe`?      | Yes              | No                  |
//!
//! Neither poisons, which makes them `!UnwindSafe` and `!RefUnwindSafe`. Shared memory IPC
//! additionally needs the mutex to be built with `with_sharing(MutexSharing::Shared)`, an
//! option not every platform implements: see [Platform support](#platform-support).
//!
//! Both kinds are generic over a [robustness marker](robustness_markers), which decides what
//! happens when the owner of a lock dies without unlocking it. The robustness marker `Standard`
//! leaves everyone else blocked forever. The marker `Robust` hands the next owner an
//! indeterminate [guard](guards) instead, giving the new owner the opportunity to repair whatever
//! the dead one left behind.
//!
//! In POSIX terms, the markers are the two values the robustness attribute can take, both set
//! through `pthread_mutexattr_setrobust`: `Standard` corresponds to `PTHREAD_MUTEX_STALLED`, the
//! default, and `Robust` to `PTHREAD_MUTEX_ROBUST`.
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
//! The next example puts a robust, process-shared mutex at some memory mapped region with
//! [`build_borrowed`](MutexBuilder::build_borrowed). We use the
//! [`memmap2`](https://crates.io/crates/memmap2) crate to create the mapping, though any
//! RAII wrapper would work.
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
//!         // The last owner died holding the lock. Repair whatever it was protecting, and only
//!         // then say so: until somebody does, every later lock fails with NotRecoverable.
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
//! If the mutex was initialised by another process, use [`BorrowedMutex::from_raw`] instead of
//! the builder:
//!
//! ```no_run
//! use posix_sync::mutex::{BorrowedMutex, RawMutexAlloc, robustness_markers::Standard};
//!
//! // A mapping of the file in which another process already initialised a mutex.
//! let file = std::fs::OpenOptions::new()
//!     .read(true)
//!     .write(true)
//!     .open("mutex.shared")?;
//! let map = memmap2::MmapRaw::map_raw(&file)?;
//!
//! // The marker has to match the robustness the initialising process picked; assume it picked
//! // Standard.
//! let mtx: BorrowedMutex<Standard> = unsafe {
//!     BorrowedMutex::from_raw(map.as_mut_ptr() as *mut RawMutexAlloc, &map)
//! };
//!
//! drop(unsafe { mtx.lock()? });
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! When you are setting up a shared mapping that carries some state, the mutex guarding that
//! state, and perhaps a condvar to wait on it with, [`RawMutexAlloc::SIZE`] bytes need to be
//! reserved for the mutex, at an offset aligned to [`RawMutexAlloc::ALIGN`].
//!
//! # Platform support
//!
//! Process sharing, robust mutexes and the `Inherit` protocol are not available everywhere. See
//! the table at the [crate root](crate#platform-support).

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

/// Cast to a pointer of this type when constructing a mutex from a raw pointer.
///
/// This is a `pthread_mutex_t` that has been made `!Unpin`, because a mutex must not be relocated
/// once it has been initialised: waiters and the kernel both refer to it by address. The two
/// associated constants describe how much room to leave for one when carving up a shared mapping
/// by hand.
#[repr(transparent)]
pub struct RawMutexAlloc {
    // the storage itself. it is only ever touched through a `*mut pthread_mutex_t`.
    #[allow(dead_code)]
    raw: pthread_mutex_t,

    /// This field is here because [`pthread_mutex_t`] is `Unpin`.
    _phantom: PhantomPinned,
}

impl RawMutexAlloc {
    /// The size, in bytes, of the underlying `pthread_mutex_t`.
    pub const SIZE: usize = size_of::<pthread_mutex_t>();

    /// The alignment a `*mut RawMutexAlloc` must satisfy.
    pub const ALIGN: usize = align_of::<pthread_mutex_t>();
}
