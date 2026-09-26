use std::cell::UnsafeCell;
use std::fmt::{self, Debug};
use std::marker::{PhantomData, Unpin};
use std::mem::MaybeUninit;

use super::builders::BarrierBuilder;
use super::{wait_on, BarrierInitError, BarrierWaitError, BarrierWaitOutcome, RawBarrierAlloc};
use crate::ffi::{self, pthread_barrier_t};
use crate::utils::{AsRawUnderlying, Sealed};

/// A barrier that owns its underlying raw barrier, which is destroyed when `OwnedBarrier` is
/// dropped. If you want to construct a barrier in shared memory for IPC, use a
/// [`BorrowedBarrier`](crate::barrier::BorrowedBarrier) instead.
///
/// Because the allocation belongs to this object and nothing outside the process can reach it, the
/// methods are safe.
pub struct OwnedBarrier {
    raw: HeapRawBarrier,

    /// The `*const UnsafeCell` is to prevent the type from being `UnwindSafe` and `RefUnwindSafe`.
    _phantom: PhantomData<*const UnsafeCell<()>>,
}

impl Sealed for OwnedBarrier {}
unsafe impl Send for OwnedBarrier {}
unsafe impl Sync for OwnedBarrier {}
impl Unpin for OwnedBarrier {}

impl AsRawUnderlying for OwnedBarrier {
    type Underlying = pthread_barrier_t;

    fn as_raw_underlying(&self) -> *mut pthread_barrier_t {
        self.raw.as_raw_underlying()
    }
}

impl Debug for OwnedBarrier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedBarrier").finish_non_exhaustive()
    }
}

impl Drop for OwnedBarrier {
    /// Calls [`pthread_barrier_destroy`](https://man7.org/linux/man-pages/man3/pthread_barrier_destroy.3p.html)
    /// on the underlying barrier.
    ///
    /// Every waiter holds a `&self`, so by the time this runs there cannot be one left.
    #[inline]
    fn drop(&mut self) {
        unsafe {
            let r = ffi::pthread_barrier_destroy(self.as_raw_underlying());
            debug_assert_eq!(r, 0);
        }
    }
}

impl OwnedBarrier {
    /// Creates a barrier with the default attributes that releases its waiters in groups of
    /// `count`. Use [`BarrierBuilder`] to set any of the others.
    ///
    /// # Errors
    /// See [`BarrierInitError`]. A `count` of zero is rejected everywhere.
    #[inline]
    pub fn new(count: u32) -> Result<Self, BarrierInitError> {
        BarrierBuilder::new().build_owned(count)
    }

    /// Takes over storage that [`BarrierBuilder::build_owned`] has just initialised successfully.
    /// Nothing else may call this, because dropping the result destroys the barrier, and
    /// destroying one that was never initialised is undefined.
    #[inline]
    pub(super) fn from_initialised(raw: HeapRawBarrier) -> Self {
        Self {
            raw,
            _phantom: PhantomData,
        }
    }

    /// Blocks until as many threads as the barrier's count, this one included, are waiting on it,
    /// then releases them all. This is
    /// [`pthread_barrier_wait`](https://man7.org/linux/man-pages/man3/pthread_barrier_wait.3p.html).
    ///
    /// The barrier is ready for the next group as soon as this one has been released, so calling
    /// `wait` again in a loop synchronises the threads at the end of every iteration.
    ///
    /// # Errors
    /// See [`BarrierWaitError`]
    #[inline]
    pub fn wait(&self) -> Result<BarrierWaitOutcome, BarrierWaitError> {
        unsafe { wait_on(self.as_raw_underlying()) }
    }

    /// Returns a pointer to the underlying `pthread_barrier_t`.
    ///
    /// The pointer stays valid until this `OwnedBarrier` is dropped.
    #[inline]
    pub fn as_raw_barrier(&self) -> *mut pthread_barrier_t {
        self.as_raw_underlying()
    }
}

/// A heap-allocated raw barrier allocation. Dropping it frees the storage without destroying the
/// barrier, which is what lets [`BarrierBuilder::build_owned`] give up on one that failed to
/// initialise.
pub(super) struct HeapRawBarrier(*mut MaybeUninit<RawBarrierAlloc>);

impl Sealed for HeapRawBarrier {}

impl AsRawUnderlying for HeapRawBarrier {
    type Underlying = pthread_barrier_t;

    fn as_raw_underlying(&self) -> *mut pthread_barrier_t {
        self.0 as *mut _
    }
}

impl HeapRawBarrier {
    pub(super) fn new() -> Self {
        let boxed_uninit = Box::new(MaybeUninit::uninit());
        let ptr = Box::into_raw(boxed_uninit);
        Self(ptr)
    }
}

impl Drop for HeapRawBarrier {
    fn drop(&mut self) {
        // # Safety
        // This is fine because the pointer came from `Box::into_raw`.
        unsafe {
            let _ = Box::from_raw(self.0);
        }
    }
}
