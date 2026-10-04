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

/// A condvar that borrows the memory for its underlying `pthread_cond_t`. The underlying condvar
/// will **not** be destroyed when `BorrowedCondvar` is dropped. It is the caller's responsibility
/// to call [`destroy`](BorrowedCondvar::destroy) when the condvar is no longer needed.
///
/// # Safety
///
/// The methods of `BorrowedCondvar` are unsafe, unlike those of
/// [`OwnedCondvar`](crate::condvar::OwnedCondvar). The underlying condvar may be in a shared
/// memory mapping, where other processes can modify it, and calling these methods can lead to
/// undefined behavior if certain invariants are not upheld. Here is a non-exhaustive list of
/// situations that lead to undefined behavior:
///
/// - Another process destroys the condvar before your process calls `destroy`. Calling any method
///   on the condvar (even `destroy` itself) is now UB.
/// - Two threads wait on the condvar at the same time with guards belonging to different mutexes.
///   An [`OwnedCondvar`](crate::condvar::OwnedCondvar) detects this and panics.
/// - The condvar is destroyed while a thread in any process is still waiting on it.
///
/// For a more thorough description of what situations can result in UB, read some of the
/// `pthread_cond_*` pages in the
/// [POSIX standard](https://pubs.opengroup.org/onlinepubs/9799919799/idx/ip.html).
pub struct BorrowedCondvar<'a> {
    raw: *mut RawCondvarAlloc,
    clock: CondvarClock,
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
    /// Constructs a `BorrowedCondvar` from an existing underlying condvar at a given
    /// location in memory.
    ///
    /// - `raw`: A pointer to an existing raw condvar. Note that, unlike
    ///   [`CondvarBuilder::build_borrowed`](crate::condvar::CondvarBuilder::build_borrowed),
    ///   this method expects that the pointee is initialized.
    ///
    /// - `_memory`: A reference to a RAII object whose lifetime determines the validity of `raw`
    ///   (e.g. a struct that manages a memory map).
    ///
    /// - `clock`: The clock the condvar was initialized with. Passing the wrong one produces the
    ///   wrong timeouts.
    ///
    /// # Safety
    /// The caller must guarantee that `raw` points to a valid, initialized [`RawCondvarAlloc`]
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
    /// on the underlying condvar. It is safe to re-initialize another condvar at this address
    /// afterwards.
    ///
    /// Failure to call this function before releasing the memory can result in resource leaks, but
    /// will not lead to undefined behavior.
    ///
    /// # Safety
    /// The caller must ensure that no thread in any process is waiting on the condvar while the
    /// call is taking place, and that `destroy` is only called once.
    ///
    /// Also, read the type-level safety section.
    #[inline]
    pub unsafe fn destroy(self) {
        unsafe {
            let r = libc::pthread_cond_destroy(self.as_raw_underlying());
            debug_assert_eq!(r, 0);
        }
    }

    /// The clock this condvar uses to resolve the deadlines of its timed waits, as passed to
    /// [`from_raw`](Self::from_raw) or set by the builder.
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

    /// Unlocks the mutex `guard` belongs to, blocks until this condvar is notified, and locks the
    /// mutex again before returning. The guard is still valid afterwards.
    ///
    /// Wakeups may be spurious, so this should be wrapped in a loop that re-checks the predicate.
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
    /// [`self.clock()`](Self::clock). The mutex is locked again on return in both cases.
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
