use std::cell::UnsafeCell;
use std::fmt::{self, Debug};
use std::marker::{PhantomData, Send, Sync, Unpin};
use std::time::Duration;

use libc::pthread_cond_t;

use super::{
    notify_all_on, notify_one_on, wait_on, wait_on_for, CondvarClock, CondvarSignalError,
    CondvarWaitError, RawCondvarAlloc, WaitOutcome,
};
use crate::mutex::guards::MutexGuard;
use crate::utils::{AsRawUnderlying, Sealed};

/// A condvar that borrows the memory for its underlying condvar. The underlying condvar will
/// **not** be destroyed when `BorrowedCondvar` is dropped. It is the caller's responsibility to
/// call [`destroy`](BorrowedCondvar::destroy) when the condvar is no longer needed.
///
/// # Safety
///
/// The methods of `BorrowedCondvar`, unlike those of [`OwnedCondvar`](crate::condvar::OwnedCondvar),
/// are unsafe. This is because the underlying condvar may be in a shared memory mapping, where it
/// may be modifiable by other processes, and calling these methods can lead to undefined behaviour
/// if certain invariants are not upheld. Here is a non-exhaustive list of some of the situations
/// that will lead to undefined behaviour:
///
/// - Another process destroys the condvar before your process calls `destroy`. Calling any method
///   on the condvar (even destroy itself) is now UB.
/// - Two threads wait on the condvar at the same time with guards belonging to different mutexes.
///   An [`OwnedCondvar`](crate::condvar::OwnedCondvar) catches this and panics, but a condvar in
///   shared memory has no cheap way to compare mutex addresses across address spaces.
/// - The condvar is destroyed while a thread in any process is still waiting on it.
///
/// For a more thorough description of what situations can result in UB, read some of the
/// `pthread_cond_*` pages in the
/// [POSIX standard](https://pubs.opengroup.org/onlinepubs/9799919799/idx/ip.html).
pub struct BorrowedCondvar<'a> {
    raw: *mut RawCondvarAlloc,
    clock: CondvarClock,

    /// The `*const UnsafeCell` is to prevent the type from being `UnwindSafe` and `RefUnwindSafe`.
    _phantom: PhantomData<(*const UnsafeCell<()>, &'a ())>,
}

impl Sealed for BorrowedCondvar<'_> {}
unsafe impl Send for BorrowedCondvar<'_> {}
unsafe impl Sync for BorrowedCondvar<'_> {}
impl Unpin for BorrowedCondvar<'_> {}

impl Clone for BorrowedCondvar<'_> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for BorrowedCondvar<'_> {}

impl AsRawUnderlying for BorrowedCondvar<'_> {
    type Underlying = pthread_cond_t;

    fn as_raw_underlying(&self) -> *mut pthread_cond_t {
        self.raw as *mut _
    }
}

impl Debug for BorrowedCondvar<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BorrowedCondvar")
            .field("raw", &self.raw)
            .field("clock", &self.clock)
            .finish()
    }
}

impl BorrowedCondvar<'_> {
    /// Constructs a `BorrowedCondvar` from an existing, initialised underlying condvar at a given
    /// location in memory.
    ///
    /// - `raw`: A pointer to an existing raw condvar. Note that, unlike
    ///   [`CondvarBuilder::build_borrowed`](crate::condvar::CondvarBuilder::build_borrowed),
    ///   this method expects that the pointee is initialised.
    ///
    /// - `_memory`: A reference to a RAII object whose lifetime determines the validity of `raw`
    ///   (e.g. a struct that manages a memory map).
    ///
    /// - `clock`: The clock the condvar was initialised with. Timed waits resolve their deadlines
    ///   against it, so naming the wrong one silently produces the wrong timeout.
    ///
    /// # Safety
    /// The caller must guarantee that `raw` points to a valid, initialised [`RawCondvarAlloc`]
    /// (a.k.a. `pthread_cond_t`).
    ///
    /// Also, read the type-level safety section.
    #[inline]
    pub unsafe fn from_raw<T>(
        raw: *mut RawCondvarAlloc,
        _memory: &T,
        clock: CondvarClock,
    ) -> BorrowedCondvar<'_> {
        BorrowedCondvar {
            raw,
            clock,
            _phantom: PhantomData,
        }
    }

    /// Calls [`pthread_cond_destroy`](https://man7.org/linux/man-pages/man3/pthread_cond_destroy.3p.html)
    /// on the underlying condvar. It is safe to re-initialise another condvar at this address
    /// afterwards.
    ///
    /// Failure to call this function before releasing the memory can result in resource leaks, but
    /// will not lead to undefined behaviour.
    ///
    /// # Safety
    /// The caller must ensure that no thread in any process is waiting on the condvar while the
    /// call is taking place, and that destroy is only called once.
    ///
    /// Also, read the type-level safety section.
    #[inline]
    pub unsafe fn destroy(self) {
        unsafe {
            let r = libc::pthread_cond_destroy(self.as_raw_underlying());
            debug_assert_eq!(r, 0);
        }
    }

    /// The clock this condvar resolves the deadlines of its timed waits against, as passed to
    /// [`from_raw`](Self::from_raw) or picked by the builder.
    #[inline]
    pub fn clock(&self) -> CondvarClock {
        self.clock
    }

    /// Wakes one thread waiting on this condvar, if there is one. This is
    /// [`pthread_cond_signal`](https://man7.org/linux/man-pages/man3/pthread_cond_signal.3p.html).
    ///
    /// # Safety
    /// Read the type-level safety section.
    ///
    /// # Errors
    /// See [`CondvarSignalError`]
    #[inline]
    pub unsafe fn notify_one(&self) -> Result<(), CondvarSignalError> {
        notify_one_on(self.as_raw_underlying())
    }

    /// Wakes every thread waiting on this condvar. This is
    /// [`pthread_cond_broadcast`](https://man7.org/linux/man-pages/man3/pthread_cond_broadcast.3p.html).
    ///
    /// # Safety
    /// Read the type-level safety section.
    ///
    /// # Errors
    /// See [`CondvarSignalError`]
    #[inline]
    pub unsafe fn notify_all(&self) -> Result<(), CondvarSignalError> {
        notify_all_on(self.as_raw_underlying())
    }

    /// Releases the mutex `guard` belongs to, blocks until this condvar is notified, and takes the
    /// mutex again before returning.
    ///
    /// Wakeups are permitted to be spurious, so this should be wrapped in a loop that re-checks
    /// the predicate.
    ///
    /// # Safety
    /// Every thread that waits on this condvar, in every process, must pass a guard belonging to
    /// the same mutex. Also, read the type-level safety section.
    ///
    /// # Errors
    /// See [`CondvarWaitError`]
    #[inline]
    pub unsafe fn wait<G>(&self, guard: &mut G) -> Result<(), CondvarWaitError>
    where
        G: MutexGuard,
    {
        wait_on(self.as_raw_underlying(), guard)
    }

    /// Like [`wait`](Self::wait), but gives up once `timeout` has elapsed on
    /// [`self.clock()`](Self::clock). The mutex is held again on return either way.
    ///
    /// # Safety
    /// The same conditions as for [`wait`](Self::wait) apply.
    ///
    /// # Errors
    /// See [`CondvarWaitError`]
    #[inline]
    pub unsafe fn wait_for<G>(
        &self,
        guard: &mut G,
        timeout: Duration,
    ) -> Result<WaitOutcome, CondvarWaitError>
    where
        G: MutexGuard,
    {
        wait_on_for(self.as_raw_underlying(), self.clock, guard, timeout)
    }

    /// Returns a pointer to the underlying `pthread_cond_t`.
    #[inline]
    pub fn as_raw_condvar(&self) -> *mut pthread_cond_t {
        self.as_raw_underlying()
    }
}
