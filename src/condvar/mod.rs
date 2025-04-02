//! Rust wrappers around POSIX condition variables.
//!
//! A condvar is always used together with a mutex: the wait releases the mutex, blocks, and
//! re-acquires it before returning. A wait therefore takes a `&mut` to one of the
//! [guards](crate::mutex::guards), which is how a caller proves it holds the lock. Spurious
//! wakeups are permitted by POSIX, so a wait should be wrapped in a loop that re-checks the
//! predicate.
//!
//! This module exposes two kinds of condvars: [`OwnedCondvar`] and [`BorrowedCondvar`].
//! Both wrap an underlying `libc::pthread_cond_t`, the condition variable object POSIX defines,
//! with the following differences:
//!
//! |                                      | **`OwnedCondvar`** | **`BorrowedCondvar`** |
//! |--------------------------------------|--------------------|-----------------------|
//! | Implements `Copy`?                   | No                 | Yes                   |
//! | Destroys underlying condvar on drop? | Always             | Never                 |
//! | Can be used for shared memory IPC?   | No                 | Yes                   |
//! | Can be used without `unsafe`?        | Yes                | No                    |
//!
//! In addition, we have the following: Suppose we have a thread waiting on
//! condvar C with mutex A. Then, a second thread attempts to wait on C with mutex B.
//!
//! - If C is an [`OwnedCondvar`], the wait with B panics, regardless of whether the
//!   wait with A has already finished or not. This is because an `OwnedCondvar`
//!   is tethered to the first mutex it is waited on with (mutex A in this example).
//!
//! - If C is a [`BorrowedCondvar`], the wait with B is undefined behaviour *if it is
//!   attempted while the wait with A is ongoing*. If the wait with A has finished by
//!   the time the wait with B is attempted, then there is no issue.
//!
//! In short: **for a given condvar, always use the same mutex**.
//!
//! Shared memory IPC
//! needs the condvar to be built with `with_sharing(CondvarSharing::Shared)`, an option not
//! every platform implements: see [Platform support](#platform-support).
//!
//!
//! Beyond sharing, each condvar is associated with a clock
//! (`Realtime` or `Monotonic`) used for tracking the passage of time
//! during `wait_for`.
//! In terms of the POSIX API, the two clocks [`CondvarClock`] offers are set through
//! `pthread_condattr_setclock`: `Realtime` is `CLOCK_REALTIME`, the wall clock and the default,
//! and `Monotonic` is `CLOCK_MONOTONIC`.
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
//!             // The mutex is released for the duration of the wait and held again on return,
//!             // so `guard` is still good on the next trip around the loop.
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
//! The next example puts a process-shared mutex, a process-shared condvar and the flag they
//! guard into one memory mapped region with [`build_borrowed`](CondvarBuilder::build_borrowed),
//! reserving [`RawCondvarAlloc::SIZE`] bytes for the condvar at an offset aligned to
//! [`RawCondvarAlloc::ALIGN`]. The waiting side would normally be
//! another process that maps the same file; a second thread stands in for it here, making the
//! same calls that process would.
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
//! // The mutex first, then the condvar, then the flag they guard, all in one shared mapping.
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
//!     // Apple platforms have no pthread_condattr_setclock, so there the timed waits below
//!     // measure their deadlines against CLOCK_REALTIME instead.
//!     #[cfg(not(target_vendor = "apple"))]
//!     let builder = builder.with_clock(CondvarClock::Monotonic);
//!     builder.build_borrowed(map.as_mut_ptr().add(cv_offset) as *mut RawCondvarAlloc, &map)
//! };
//!
//! let flag = unsafe { map.as_mut_ptr().add(flag_offset) } as *mut u32;
//! unsafe { flag.write_volatile(0) };
//!
//! // Raw pointers are not Send, so the flag's address crosses to the waiting thread as a usize.
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
//! If the condvar was initialised by another process, use [`BorrowedCondvar::from_raw`] instead
//! of the builder. There is no portable way to ask a condvar which clock it was built with, so
//! `from_raw` trusts the caller, and naming the wrong one silently produces the
//! wrong timeouts:
//!
//! ```no_run
//! use posix_sync::condvar::{BorrowedCondvar, CondvarClock, RawCondvarAlloc};
//! use posix_sync::mutex::{BorrowedMutex, RawMutexAlloc, robustness_markers::Standard};
//! # fn align_up(offset: usize, align: usize) -> usize { (offset + align - 1) & !(align - 1) }
//!
//! // A mapping of the file in which another process already initialised a mutex and a condvar,
//! // laid out as in the previous example.
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
//! // The clock has to match the one the initialising process picked; assume it kept the
//! // default.
//! let cv = unsafe {
//!     BorrowedCondvar::from_raw(
//!         map.as_mut_ptr().add(cv_offset) as *mut RawCondvarAlloc,
//!         &map,
//!         CondvarClock::Realtime,
//!     )
//! };
//!
//! let guard = unsafe { mtx.lock()? };
//! // ... update the state the waiters over there re-check ...
//! drop(guard);
//! unsafe { cv.notify_all()? };
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Platform support
//!
//! Selecting a clock is not available everywhere: where `CondvarBuilder::with_clock` and
//! `CondvarClock::Monotonic` are missing, a condvar always measures its timed waits against
//! `CLOCK_REALTIME`. Process sharing is missing on some platforms too. See the table at the
//! [crate root](crate#platform-support).

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

/// Cast to a pointer of this type when constructing a condvar from a raw pointer.
///
/// This is a `pthread_cond_t` that has been made `!Unpin`, because a condvar must not be
/// relocated once it has been initialised. The two associated constants describe how much room to
/// leave for one when carving up a shared mapping by hand.
#[repr(transparent)]
pub struct RawCondvarAlloc {
    // the storage itself. it is only ever touched through a `*mut pthread_cond_t`.
    #[allow(dead_code)]
    raw: pthread_cond_t,

    /// This field is here because [`pthread_cond_t`] is `Unpin`.
    _phantom: PhantomPinned,
}

impl RawCondvarAlloc {
    /// The size, in bytes, of the underlying `pthread_cond_t`.
    pub const SIZE: usize = size_of::<pthread_cond_t>();

    /// The alignment a `*mut RawCondvarAlloc` must satisfy.
    pub const ALIGN: usize = align_of::<pthread_cond_t>();
}

/// The clock a condvar resolves the deadlines of its timed waits against.
/// See [`pthread_condattr_setclock`](https://man7.org/linux/man-pages/man3/pthread_condattr_setclock.3p.html)
/// for more information.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum CondvarClock {
    /// `CLOCK_REALTIME`. This is what POSIX specifies as the default, and it means
    /// that setting the system time moves every outstanding deadline with it.
    #[default]
    Realtime,
    /// `CLOCK_MONOTONIC`. Prefer this one unless a
    /// deadline genuinely refers to a wall clock time.
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
    /// The wait returned before the deadline. This does not promise a notification actually
    /// happened, only that the deadline is not why the wait ended: check the predicate.
    Notified,
    /// The deadline passed. The mutex is locked again either way.
    TimedOut,
}

/// The body of every `wait`, shared by the owned and borrowed condvars.
///
/// # Safety
/// `cond` must point at an initialised condvar, and `guard` must belong to the same mutex every
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
/// The same conditions as for [`wait_on`] apply, and `clock` must be the clock `cond` was built
/// with. Resolving the deadline against any other clock quietly produces the wrong timeout.
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
/// `cond` must point at an initialised condvar.
unsafe fn notify_one_on(cond: *mut pthread_cond_t) -> Result<(), CondvarSignalError> {
    match libc::pthread_cond_signal(cond) {
        0 => Ok(()),
        e => Err(CondvarSignalError::from(e)),
    }
}

/// The body of every `notify_all`, shared by the owned and borrowed condvars.
///
/// # Safety
/// `cond` must point at an initialised condvar.
unsafe fn notify_all_on(cond: *mut pthread_cond_t) -> Result<(), CondvarSignalError> {
    match libc::pthread_cond_broadcast(cond) {
        0 => Ok(()),
        e => Err(CondvarSignalError::from(e)),
    }
}
