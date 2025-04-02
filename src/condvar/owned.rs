use std::cell::UnsafeCell;
use std::fmt::{self, Debug};
use std::marker::{PhantomData, Unpin};
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use libc::pthread_cond_t;

use super::builders::CondvarBuilder;
use super::{
    notify_all_on, notify_one_on, wait_on, wait_on_for, CondvarClock, CondvarSignalError,
    CondvarWaitError, RawCondvarAlloc, WaitOutcome,
};
use crate::mutex::guards::MutexGuard;
use crate::utils::{AsRawUnderlying, Sealed};

/// A condvar that owns its underlying raw condvar, which is destroyed when `OwnedCondvar` is
/// dropped. If you want to construct a condvar in shared memory for IPC, use a
/// [`BorrowedCondvar`](crate::condvar::BorrowedCondvar) instead.
///
/// The methods are safe because the allocation belongs to this object, and because the POSIX
/// invariant that every concurrent waiter passes a guard belonging to the same mutex is checked
/// at runtime rather than left to the caller.
pub struct OwnedCondvar {
    raw: HeapRawCondvar,
    clock: CondvarClock,

    /// The address of the mutex the first waiter used, or 0 if there has not been one yet.
    mutex: AtomicUsize,

    /// The `*const UnsafeCell` is to prevent the type from being `UnwindSafe` and `RefUnwindSafe`.
    _phantom: PhantomData<*const UnsafeCell<()>>,
}

impl Sealed for OwnedCondvar {}
unsafe impl Send for OwnedCondvar {}
unsafe impl Sync for OwnedCondvar {}
impl Unpin for OwnedCondvar {}

impl AsRawUnderlying for OwnedCondvar {
    type Underlying = pthread_cond_t;

    fn as_raw_underlying(&self) -> *mut pthread_cond_t {
        self.raw.as_raw_underlying()
    }
}

impl Debug for OwnedCondvar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedCondvar")
            .field("clock", &self.clock)
            .finish_non_exhaustive()
    }
}

impl Default for OwnedCondvar {
    /// Equivalent to `CondvarBuilder::new().build_owned()`.
    fn default() -> Self {
        CondvarBuilder::new().build_owned()
    }
}

impl Drop for OwnedCondvar {
    /// Calls [`pthread_cond_destroy`](https://man7.org/linux/man-pages/man3/pthread_cond_destroy.3p.html)
    /// on the underlying condvar.
    ///
    /// Every waiter holds a `&self`, so by the time this runs there cannot be one left.
    #[inline]
    fn drop(&mut self) {
        unsafe {
            let r = libc::pthread_cond_destroy(self.as_raw_underlying());
            debug_assert_eq!(r, 0);
        }
    }
}

impl OwnedCondvar {
    /// Creates a condvar with the default attributes. Use [`CondvarBuilder`] to set any of the
    /// others.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates the storage without initialising it. Only [`CondvarBuilder::build_owned`], which
    /// initialises it immediately afterwards, may call this.
    #[inline]
    pub(super) fn new_uninit(clock: CondvarClock) -> Self {
        Self {
            raw: HeapRawCondvar::new(),
            clock,
            mutex: AtomicUsize::new(0),
            _phantom: PhantomData,
        }
    }

    /// The clock this condvar resolves the deadlines of its timed waits against.
    #[inline]
    pub fn clock(&self) -> CondvarClock {
        self.clock
    }

    /// Wakes one thread waiting on this condvar, if there is one. This is
    /// [`pthread_cond_signal`](https://man7.org/linux/man-pages/man3/pthread_cond_signal.3p.html).
    ///
    /// # Errors
    /// See [`CondvarSignalError`]
    #[inline]
    pub fn notify_one(&self) -> Result<(), CondvarSignalError> {
        unsafe { notify_one_on(self.as_raw_underlying()) }
    }

    /// Wakes every thread waiting on this condvar. This is
    /// [`pthread_cond_broadcast`](https://man7.org/linux/man-pages/man3/pthread_cond_broadcast.3p.html).
    ///
    /// # Errors
    /// See [`CondvarSignalError`]
    #[inline]
    pub fn notify_all(&self) -> Result<(), CondvarSignalError> {
        unsafe { notify_all_on(self.as_raw_underlying()) }
    }

    /// Releases the mutex `guard` belongs to, blocks until this condvar is notified, and takes the
    /// mutex again before returning. The guard is still valid afterwards, so the critical section
    /// simply proceeds.
    ///
    /// Wakeups are permitted to be spurious, so this should be wrapped in a loop that re-checks
    /// the predicate:
    ///
    /// ```no_run
    /// # use posix_sync::condvar::OwnedCondvar;
    /// # use posix_sync::mutex::{OwnedMutex, robustness_markers::Standard};
    /// # fn predicate_holds() -> bool { true }
    /// # let mtx = OwnedMutex::<Standard>::new();
    /// # let cv = OwnedCondvar::new();
    /// let mut guard = mtx.lock().unwrap();
    /// while !predicate_holds() {
    ///     cv.wait(&mut guard).unwrap();
    /// }
    /// ```
    ///
    /// # Panics
    /// Panics if a previous wait on this condvar used a guard belonging to a different mutex.
    ///
    /// # Errors
    /// See [`CondvarWaitError`]
    #[inline]
    pub fn wait<G>(&self, guard: &mut G) -> Result<(), CondvarWaitError>
    where
        G: MutexGuard,
    {
        self.bind_to_mutex(guard);
        unsafe { wait_on(self.as_raw_underlying(), guard) }
    }

    /// Like [`wait`](Self::wait), but gives up once `timeout` has elapsed on
    /// [`self.clock()`](Self::clock). The mutex is held again on return either way.
    ///
    /// # Panics
    /// Panics if a previous wait on this condvar used a guard belonging to a different mutex.
    ///
    /// # Errors
    /// See [`CondvarWaitError`]
    #[inline]
    pub fn wait_for<G>(
        &self,
        guard: &mut G,
        timeout: Duration,
    ) -> Result<WaitOutcome, CondvarWaitError>
    where
        G: MutexGuard,
    {
        self.bind_to_mutex(guard);
        unsafe { wait_on_for(self.as_raw_underlying(), self.clock, guard, timeout) }
    }

    /// Returns a pointer to the underlying `pthread_cond_t`.
    ///
    /// The pointer stays valid until this `OwnedCondvar` is dropped.
    #[inline]
    pub fn as_raw_condvar(&self) -> *mut pthread_cond_t {
        self.as_raw_underlying()
    }

    /// Records which mutex this condvar is being waited on with, and rejects any later attempt to
    /// use a different one.
    fn bind_to_mutex<G: MutexGuard>(&self, guard: &G) {
        let mutex = guard.as_raw_underlying() as usize;
        match self
            .mutex
            .compare_exchange(0, mutex, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {}
            Err(bound) if bound == mutex => {}
            Err(_) => panic!("this condvar is already being waited on with a different mutex"),
        }
    }
}

/// A heap-allocated raw condvar allocation.
struct HeapRawCondvar(*mut MaybeUninit<RawCondvarAlloc>);

impl Sealed for HeapRawCondvar {}

impl AsRawUnderlying for HeapRawCondvar {
    type Underlying = pthread_cond_t;

    fn as_raw_underlying(&self) -> *mut pthread_cond_t {
        self.0 as *mut _
    }
}

impl HeapRawCondvar {
    fn new() -> Self {
        let boxed_uninit = Box::new(MaybeUninit::uninit());
        let ptr = Box::into_raw(boxed_uninit);
        Self(ptr)
    }
}

impl Drop for HeapRawCondvar {
    fn drop(&mut self) {
        // # Safety
        // This is fine because the pointer came from `Box::into_raw`.
        unsafe {
            let _ = Box::from_raw(self.0);
        }
    }
}
