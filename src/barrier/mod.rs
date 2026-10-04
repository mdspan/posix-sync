//! Rust wrappers around POSIX barriers.
//!
//! A barrier holds back every thread that waits on it until a fixed number of them have arrived,
//! then releases the whole group at once. That number is the count the barrier is built with.
//! Releasing a group resets the barrier, so the same one can mark the end of every phase of a
//! loop.
//!
//! This module exposes two kinds of barriers: [`OwnedBarrier`] and [`BorrowedBarrier`].
//!
//! |                                      | **`OwnedBarrier`** | **`BorrowedBarrier`** |
//! |--------------------------------------|--------------------|-----------------------|
//! | Implements `Copy`?                   | No                 | Yes                   |
//! | Destroys underlying barrier on drop? | Always             | Never                 |
//! | Can be used for shared memory IPC?   | No                 | Yes                   |
//! | Can be used without `unsafe`?        | Yes                | No                    |
//!
//! Shared memory IPC additionally needs the barrier to be built with
//! `with_sharing(BarrierSharing::Shared)`, an option not every platform implements: see
//! [Platform support](#platform-support).
//!
//! Of each group that is released, exactly one thread gets [`BarrierWaitOutcome::Serial`] back
//! from its wait and the others get [`BarrierWaitOutcome::NonSerial`]. POSIX leaves unspecified
//! which thread that is. The serial thread can run work that has to happen once per phase, such
//! as merging the results the phase produced.
//!
//! Barriers are constructed with [`BarrierBuilder`]:
//!
//! ```
//! use std::sync::atomic::{AtomicU32, Ordering};
//! use std::thread;
//!
//! use posix_sync::barrier::{BarrierBuilder, BarrierWaitOutcome};
//!
//! const THREADS: u32 = 4;
//!
//! let barrier = BarrierBuilder::new().build_owned(THREADS)?;
//! let arrived = AtomicU32::new(0);
//! let serial = AtomicU32::new(0);
//!
//! thread::scope(|s| {
//!     for _ in 0..THREADS {
//!         s.spawn(|| {
//!             arrived.fetch_add(1, Ordering::Relaxed);
//!
//!             let outcome = barrier.wait().unwrap();
//!             if outcome == BarrierWaitOutcome::Serial {
//!                 serial.fetch_add(1, Ordering::Relaxed);
//!             }
//!
//!             // Nobody gets past the barrier until everybody has reached it.
//!             assert_eq!(arrived.load(Ordering::Relaxed), THREADS);
//!         });
//!     }
//! });
//!
//! assert_eq!(serial.load(Ordering::Relaxed), 1);
//! # Ok::<(), posix_sync::barrier::BarrierInitError>(())
//! ```
//!
//! The following example puts a process-shared barrier and a slot for each of its two parties into
//! one memory mapped region with [`build_borrowed`](BarrierBuilder::build_borrowed), reserving
//! [`RawBarrierAlloc::SIZE`] bytes for the barrier at an offset aligned to
//! [`RawBarrierAlloc::ALIGN`]. Each party fills in its own slot before the barrier and reads the
//! other's after it. In practice the second party would be another process that maps the same
//! file. Here a second thread makes the same calls.
//!
//! ```
//! # #[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use std::mem::size_of;
//! use std::thread;
//!
//! use posix_sync::barrier::{BarrierBuilder, BarrierSharing, RawBarrierAlloc};
//!
//! fn align_up(offset: usize, align: usize) -> usize {
//!     (offset + align - 1) & !(align - 1)
//! }
//!
//! // Both slots first, then the barrier, in one shared mapping.
//! let barrier_offset = align_up(2 * size_of::<u64>(), RawBarrierAlloc::ALIGN);
//!
//! let file = tempfile::tempfile()?;
//! file.set_len((barrier_offset + RawBarrierAlloc::SIZE) as u64)?;
//! let map = memmap2::MmapRaw::map_raw(&file)?;
//!
//! let barrier = unsafe {
//!     BarrierBuilder::new()
//!         .with_sharing(BarrierSharing::Shared)
//!         .build_borrowed(
//!             map.as_mut_ptr().add(barrier_offset) as *mut RawBarrierAlloc,
//!             &map,
//!             2,
//!         )?
//! };
//!
//! let slots = map.as_mut_ptr() as *mut u64;
//!
//! // Raw pointers are not Send, so the slots' address crosses to the other thread as a usize.
//! let slots_addr = slots as usize;
//! thread::scope(|s| {
//!     s.spawn(move || {
//!         let slots = slots_addr as *mut u64;
//!         unsafe { slots.add(1).write_volatile(2) };
//!         unsafe { barrier.wait() }.unwrap();
//!         assert_eq!(unsafe { slots.read_volatile() }, 1);
//!     });
//!
//!     unsafe { slots.write_volatile(1) };
//!     unsafe { barrier.wait() }.unwrap();
//!     assert_eq!(unsafe { slots.add(1).read_volatile() }, 2);
//! });
//!
//! unsafe { barrier.destroy() };
//! # Ok(())
//! # }
//! # #[cfg(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))]
//! # fn main() {}
//! ```
//!
//! If the barrier was initialized by another process or thread, use [`BorrowedBarrier::from_raw`]
//! instead of the builder. `from_raw` takes no count, since the count is stored in the barrier
//! itself:
//!
//! ```no_run
//! use posix_sync::barrier::{BorrowedBarrier, RawBarrierAlloc};
//!
//! // A mapping of the file in which another process already initialized a barrier.
//! let file = std::fs::OpenOptions::new()
//!     .read(true)
//!     .write(true)
//!     .open("barrier.shared")?;
//! let map = memmap2::MmapRaw::map_raw(&file)?;
//!
//! let barrier =
//!     unsafe { BorrowedBarrier::from_raw(map.as_mut_ptr() as *mut RawBarrierAlloc, &map) };
//!
//! unsafe { barrier.wait()? };
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Platform support
//!
//! See the table at the [crate root](crate#platform-support).

use std::marker::PhantomPinned;
use std::mem::{align_of, size_of};

use crate::ffi::{self, pthread_barrier_t};

mod errors;
pub use errors::*;

mod owned;
pub use owned::*;

mod borrowed;
pub use borrowed::*;

pub mod builders;
pub use builders::BarrierBuilder;

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
pub use builders::BarrierSharing;

/// Cast to a pointer of this type when constructing a barrier from a raw pointer.
///
/// This is a `pthread_barrier_t` that has been made `!Unpin`, because a barrier must not be
/// relocated once it has been initialized. The two associated constants describe how the object
/// should be placed in memory.
#[repr(transparent)]
pub struct RawBarrierAlloc {
    #[allow(dead_code)]
    raw: pthread_barrier_t,
    _phantom: PhantomPinned,
}

impl RawBarrierAlloc {
    /// The size, in bytes, of the underlying `pthread_barrier_t`.
    pub const SIZE: usize = size_of::<pthread_barrier_t>();

    /// The alignment a `*mut RawBarrierAlloc` must satisfy.
    pub const ALIGN: usize = align_of::<pthread_barrier_t>();
}

/// The result of a successful wait. POSIX picks out one thread of every group the barrier
/// releases, and this says whether the caller was that thread.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum BarrierWaitOutcome {
    /// `pthread_barrier_wait` returned `PTHREAD_BARRIER_SERIAL_THREAD`: this thread was picked
    /// out of the group being released. Exactly one thread per group gets this.
    Serial,
    /// `pthread_barrier_wait` returned zero, as it does for every thread in the group but one.
    NonSerial,
}

/// The body of every `wait`, shared by the owned and borrowed barriers.
///
/// # Safety
/// `barrier` must point at an initialized barrier.
unsafe fn wait_on(barrier: *mut pthread_barrier_t) -> Result<BarrierWaitOutcome, BarrierWaitError> {
    match ffi::pthread_barrier_wait(barrier) {
        0 => Ok(BarrierWaitOutcome::NonSerial),
        ffi::PTHREAD_BARRIER_SERIAL_THREAD => Ok(BarrierWaitOutcome::Serial),
        e => Err(BarrierWaitError::from(e)),
    }
}
