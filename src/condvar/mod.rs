//! Rust wrappers around POSIX condition variables.
//!
//! A wait on a condvar releases the mutex, blocks, and locks the mutex again before returning, so
//! every wait takes a `&mut` to one of the mutex [guards](crate::mutex::guards). POSIX permits
//! spurious wakeups, so a wait should be wrapped in a loop that re-checks the predicate.
//!
//! This module exposes two kinds of condvars: [`OwnedCondvar`] and [`BorrowedCondvar`].
//!
//! |                                      | **`OwnedCondvar`** | **`BorrowedCondvar`** |
//! |--------------------------------------|--------------------|-----------------------|
//! | Implements `Copy`?                   | No                 | Yes                   |
//! | Destroys underlying condvar on drop? | Always             | Never                 |
//! | Can be used for shared memory IPC?   | No                 | Yes                   |
//! | Can be used without `unsafe`?        | Yes                | No                    |
//!
//! Suppose a thread is waiting on condvar C with mutex A, and a second thread then waits on C with
//! mutex B.
//!
//! - If C is an [`OwnedCondvar`], the wait with B panics, whether or not the wait with A has
//!   finished. An `OwnedCondvar` is bound to the first mutex it is waited on with (A here).
//!
//! - If C is a [`BorrowedCondvar`], the wait with B is undefined behavior *if the wait with A is
//!   still ongoing*. If the wait with A has finished, the wait with B is fine.
//!
//! In short: **for a given condvar, always use the same mutex**.
//!
//! Shared memory IPC additionally needs the condvar to be built with
//! `with_sharing(CondvarSharing::Shared)`, an option not every platform implements: see
//! [Platform support](#platform-support).
//!
//! Each condvar has a [clock](CondvarClock), which corresponds to the POSIX clock attribute (see
//! [`pthread_condattr_setclock`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_condattr_setclock.html))
//! and is used to resolve the deadlines of timed waits. `Realtime` is `CLOCK_REALTIME`, the
//! default, and `Monotonic` is `CLOCK_MONOTONIC`.
//!
//! Condvars are constructed with [`CondvarBuilder`]:
//!
//! ```
//! use posix_sync::condvar::{CondvarBuilder, WaitOutcome};
//! use posix_sync::mutex::{OwnedMutex, robustness_markers::Standard};
//! use std::sync::atomic::{AtomicBool, Ordering};
//! use std::thread;
//! use std::time::Duration;
//!
//! let mtx = OwnedMutex::<Standard>::new();
//! let cv = CondvarBuilder::new().build_owned();
//! let ready = AtomicBool::new(false);
//!
//! thread::scope(|s| {
//!     s.spawn(|| {
//!         let mut guard = mtx.lock().unwrap();
//!         while !ready.load(Ordering::Acquire) {
//!             // The mutex is locked again on return, so `guard` is still valid here.
//!             let outcome = cv.wait_for(&mut guard, Duration::from_secs(5)).unwrap();
//!             assert_eq!(outcome, WaitOutcome::Notified);
//!         }
//!     });
//!
//!     let guard = mtx.lock().unwrap();
//!     ready.store(true, Ordering::Release);
//!     drop(guard);
//!     cv.notify_all().unwrap();
//! });
//! ```
//!
//! The following example puts a process-shared mutex, a process-shared condvar and a flag into
//! one memory mapped region with [`build_borrowed`](CondvarBuilder::build_borrowed). The condvar
//! takes [`RawCondvarAlloc::SIZE`] bytes at an offset aligned to [`RawCondvarAlloc::ALIGN`]. Here,
//! we use a second thread to represent what would be another process.
//!
//! ```
//! # #[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use std::mem::{align_of, size_of};
//! use std::thread;
//! use std::time::Duration;
//!
//! use posix_sync::condvar::{CondvarBuilder, CondvarSharing, RawCondvarAlloc, WaitOutcome};
//! # #[cfg(not(target_vendor = "apple"))]
//! # use posix_sync::condvar::CondvarClock;
//! use posix_sync::mutex::{
//!     MutexBuilder, MutexSharing, RawMutexAlloc, robustness_markers::Standard,
//! };
//!
//! fn align_up(offset: usize, align: usize) -> usize {
//!     (offset + align - 1) & !(align - 1)
//! }
//!
//! // Layout: the mutex, then the condvar, then the flag.
//! let cv_offset = align_up(RawMutexAlloc::SIZE, RawCondvarAlloc::ALIGN);
//! let flag_offset = align_up(cv_offset + RawCondvarAlloc::SIZE, align_of::<u32>());
//!
//! let file = tempfile::tempfile()?;
//! file.set_len((flag_offset + size_of::<u32>()) as u64)?;
//! let map = memmap2::MmapRaw::map_raw(&file)?;
//!
//! let mtx = unsafe {
//!     MutexBuilder::<Standard>::new()
//!         .with_sharing(MutexSharing::Shared)
//!         .build_borrowed(map.as_mut_ptr() as *mut RawMutexAlloc, &map)
//! };
//! let cv = unsafe {
//!     let builder = CondvarBuilder::new().with_sharing(CondvarSharing::Shared);
//!     // Apple platforms lack pthread_condattr_setclock and always use CLOCK_REALTIME.
//!     #[cfg(not(target_vendor = "apple"))]
//!     let builder = builder.with_clock(CondvarClock::Monotonic);
//!     builder.build_borrowed(map.as_mut_ptr().add(cv_offset) as *mut RawCondvarAlloc, &map)
//! };
//!
//! let flag = unsafe { map.as_mut_ptr().add(flag_offset) } as *mut u32;
//! unsafe { flag.write_volatile(0) };
//!
//! // Raw pointers are not Send, so the address is passed as a usize.
//! let flag_addr = flag as usize;
//! thread::scope(|s| {
//!     s.spawn(move || {
//!         let flag = flag_addr as *mut u32;
//!         let mut guard = unsafe { mtx.lock() }.unwrap();
//!         while unsafe { flag.read_volatile() } == 0 {
//!             let outcome =
//!                 unsafe { cv.wait_for(&mut guard, Duration::from_secs(5)) }.unwrap();
//!             assert_eq!(outcome, WaitOutcome::Notified);
//!         }
//!     });
//!
//!     let guard = unsafe { mtx.lock() }.unwrap();
//!     unsafe { flag.write_volatile(1) };
//!     drop(guard);
//!     unsafe { cv.notify_all() }.unwrap();
//! });
//!
//! unsafe {
//!     cv.destroy();
//!     mtx.destroy();
//! }
//! # Ok(())
//! # }
//! # #[cfg(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))]
//! # fn main() {}
//! ```
//!
//! If the condvar was initialized by another process or thread, use [`BorrowedCondvar::from_raw`]
//! instead of the builder. POSIX provides no way to query the clock of an initialized condvar, so
//! the caller passes it in, and passing the wrong one produces the wrong timeouts:
//!
//! ```no_run
//! use posix_sync::condvar::{BorrowedCondvar, CondvarClock, RawCondvarAlloc};
//! use posix_sync::mutex::{BorrowedMutex, RawMutexAlloc, robustness_markers::Standard};
//! # fn align_up(offset: usize, align: usize) -> usize { (offset + align - 1) & !(align - 1) }
//!
//! // Another process has initialized a mutex and a condvar in this file, laid out as in the
//! // previous example.
//! let file = std::fs::OpenOptions::new()
//!     .read(true)
//!     .write(true)
//!     .open("condvar.shared")?;
//! let map = memmap2::MmapRaw::map_raw(&file)?;
//! let cv_offset = align_up(RawMutexAlloc::SIZE, RawCondvarAlloc::ALIGN);
//!
//! let mtx: BorrowedMutex<Standard> = unsafe {
//!     BorrowedMutex::from_raw(map.as_mut_ptr() as *mut RawMutexAlloc, &map)
//! };
//!
//! // The clock must match the one the condvar was initialized with; here, the default.
//! let cv = unsafe {
//!     BorrowedCondvar::from_raw(
//!         map.as_mut_ptr().add(cv_offset) as *mut RawCondvarAlloc,
//!         &map,
//!         CondvarClock::Realtime,
//!     )
//! };
//!
//! let guard = unsafe { mtx.lock()? };
//! // ... update the shared state ...
//! drop(guard);
//! unsafe { cv.notify_all()? };
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Platform support
//!
//! See the table at the [crate root](crate#platform-support).

use std::marker::PhantomPinned;
use std::mem::{align_of, size_of};
use std::time::Duration;

use libc::{clockid_t, pthread_cond_t};

use crate::mutex::guards::MutexGuard;
use crate::utils::deadline_from_now;

mod errors;
pub use errors::*;

mod owned;
pub use owned::*;

mod borrowed;
pub use borrowed::*;

pub mod builders;
pub use builders::CondvarBuilder;

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
pub use builders::CondvarSharing;

/// A `pthread_cond_t` that has been made `!Unpin`.
/// Cast to a pointer of this type when constructing a condvar from a raw pointer.
#[repr(transparent)]
pub struct RawCondvarAlloc {
    #[allow(dead_code)]
    raw: pthread_cond_t,
    _phantom: PhantomPinned,
}

impl RawCondvarAlloc {
    /// The size, in bytes, of the underlying `pthread_cond_t`.
    pub const SIZE: usize = size_of::<pthread_cond_t>();

    /// The alignment a `*mut RawCondvarAlloc` must satisfy.
    pub const ALIGN: usize = align_of::<pthread_cond_t>();
}

/// The clock a condvar uses to resolve the deadlines of its timed waits.
/// See [`pthread_condattr_setclock`](https://man7.org/linux/man-pages/man3/pthread_condattr_setclock.3p.html)
/// for more information.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum CondvarClock {
    /// `CLOCK_REALTIME`, the POSIX default. Setting the system time moves every outstanding
    /// deadline with it.
    #[default]
    Realtime,
    /// `CLOCK_MONOTONIC`. Prefer this unless a deadline refers to a wall clock time.
    ///
    /// Not available on Apple platforms, which do not implement `pthread_condattr_setclock`.
    #[cfg_attr(docsrs, doc(cfg(not(target_vendor = "apple"))))]
    #[cfg(not(target_vendor = "apple"))]
    Monotonic,
}

impl From<CondvarClock> for clockid_t {
    fn from(value: CondvarClock) -> Self {
        match value {
            CondvarClock::Realtime => libc::CLOCK_REALTIME,
            #[cfg(not(target_vendor = "apple"))]
            CondvarClock::Monotonic => libc::CLOCK_MONOTONIC,
        }
    }
}

/// Why a timed wait stopped waiting.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum WaitOutcome {
    /// The wait returned before the deadline. The wakeup may be spurious, so check the predicate.
    Notified,
    /// The deadline passed. The mutex is locked again in both cases.
    TimedOut,
}

/// The body of every `wait`, shared by the owned and borrowed condvars.
///
/// # Safety
/// `cond` must point at an initialized condvar, and `guard` must belong to the same mutex every
/// other waiter on `cond` is using.
unsafe fn wait_on<G>(cond: *mut pthread_cond_t, guard: &mut G) -> Result<(), CondvarWaitError>
where
    G: MutexGuard,
{
    match libc::pthread_cond_wait(cond, guard.as_raw_underlying()) {
        0 => Ok(()),
        e => Err(CondvarWaitError::from(e)),
    }
}

/// The body of every `wait_for`, shared by the owned and borrowed condvars.
///
/// # Safety
/// The same conditions as for [`wait_on`] apply, and `clock` must be the clock `cond` was
/// initialized with.
unsafe fn wait_on_for<G>(
    cond: *mut pthread_cond_t,
    clock: CondvarClock,
    guard: &mut G,
    timeout: Duration,
) -> Result<WaitOutcome, CondvarWaitError>
where
    G: MutexGuard,
{
    let deadline = deadline_from_now(clock.into(), timeout);
    match libc::pthread_cond_timedwait(cond, guard.as_raw_underlying(), &deadline) {
        0 => Ok(WaitOutcome::Notified),
        libc::ETIMEDOUT => Ok(WaitOutcome::TimedOut),
        e => Err(CondvarWaitError::from(e)),
    }
}

/// The body of every `notify_one`, shared by the owned and borrowed condvars.
///
/// # Safety
/// `cond` must point at an initialized condvar.
unsafe fn notify_one_on(cond: *mut pthread_cond_t) -> Result<(), CondvarSignalError> {
    match libc::pthread_cond_signal(cond) {
        0 => Ok(()),
        e => Err(CondvarSignalError::from(e)),
    }
}

/// The body of every `notify_all`, shared by the owned and borrowed condvars.
///
/// # Safety
/// `cond` must point at an initialized condvar.
unsafe fn notify_all_on(cond: *mut pthread_cond_t) -> Result<(), CondvarSignalError> {
    match libc::pthread_cond_broadcast(cond) {
        0 => Ok(()),
        e => Err(CondvarSignalError::from(e)),
    }
}
