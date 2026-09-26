use std::cell::UnsafeCell;
use std::fmt::{self, Debug};
use std::marker::{PhantomData, Send, Sync, Unpin};

use super::{wait_on, BarrierWaitError, BarrierWaitOutcome, RawBarrierAlloc};
use crate::ffi::{self, pthread_barrier_t};
use crate::utils::{AsRawUnderlying, Sealed};

/// A barrier that borrows the memory for its underlying barrier. The underlying barrier will
/// **not** be destroyed when `BorrowedBarrier` is dropped. It is the caller's responsibility to
/// call [`destroy`](BorrowedBarrier::destroy) when the barrier is no longer needed.
///
/// # Safety
///
/// The methods of `BorrowedBarrier`, unlike those of
/// [`OwnedBarrier`](crate::barrier::OwnedBarrier), are unsafe. This is because the underlying
/// barrier may be in a shared memory mapping, where it may be modifiable by other processes, and
/// calling these methods can lead to undefined behaviour if certain invariants are not upheld. Here
/// is a non-exhaustive list of some of the situations that will lead to undefined behaviour:
///
/// - Another process destroys the barrier before your process calls `destroy`. Calling any method
///   on the barrier (even destroy itself) is now UB.
/// - The barrier is destroyed while a thread in any process is still waiting on it.
/// - Another process initialises a barrier at the same address while a thread is waiting on this
///   one.
///
/// There is also no recovery from a process that dies before it reaches the barrier: POSIX gives a
/// barrier no way to report that a member of the group is gone, and the rest of the group simply
/// blocks forever.
///
/// For a more thorough description of what situations can result in UB, read some of the
/// `pthread_barrier_*` pages in the
/// [POSIX standard](https://pubs.opengroup.org/onlinepubs/9799919799/idx/ip.html).
pub struct BorrowedBarrier<'a> {
    raw: *mut RawBarrierAlloc,

    /// The `*const UnsafeCell` is to prevent the type from being `UnwindSafe` and `RefUnwindSafe`.
    _phantom: PhantomData<(*const UnsafeCell<()>, &'a ())>,
}

impl Sealed for BorrowedBarrier<'_> {}
unsafe impl Send for BorrowedBarrier<'_> {}
unsafe impl Sync for BorrowedBarrier<'_> {}
impl Unpin for BorrowedBarrier<'_> {}

impl Clone for BorrowedBarrier<'_> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for BorrowedBarrier<'_> {}

impl AsRawUnderlying for BorrowedBarrier<'_> {
    type Underlying = pthread_barrier_t;

    fn as_raw_underlying(&self) -> *mut pthread_barrier_t {
        self.raw as *mut _
    }
}

impl Debug for BorrowedBarrier<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BorrowedBarrier")
            .field("raw", &self.raw)
            .finish()
    }
}

impl BorrowedBarrier<'_> {
    /// Constructs a `BorrowedBarrier` from an existing, initialised underlying barrier at a given
    /// location in memory.
    ///
    /// - `raw`: A pointer to an existing raw barrier. Note that, unlike
    ///   [`BarrierBuilder::build_borrowed`](crate::barrier::BarrierBuilder::build_borrowed),
    ///   this method expects that the pointee is initialised.
    ///
    /// - `_memory`: A reference to a RAII object whose lifetime determines the validity of `raw`
    ///   (e.g. a struct that manages a memory map).
    ///
    /// # Safety
    /// The caller must guarantee that `raw` points to a valid, initialised [`RawBarrierAlloc`]
    /// (a.k.a. `pthread_barrier_t`).
    ///
    /// Also, read the type-level safety section.
    #[inline]
    pub unsafe fn from_raw<T>(raw: *mut RawBarrierAlloc, _memory: &T) -> BorrowedBarrier<'_> {
        BorrowedBarrier {
            raw,
            _phantom: PhantomData,
        }
    }

    /// Calls [`pthread_barrier_destroy`](https://man7.org/linux/man-pages/man3/pthread_barrier_destroy.3p.html)
    /// on the underlying barrier. It is safe to re-initialise another barrier at this address
    /// afterwards.
    ///
    /// Failure to call this function before releasing the memory can result in resource leaks, but
    /// will not lead to undefined behaviour.
    ///
    /// # Safety
    /// The caller must ensure that no thread in any process is waiting on the barrier while the
    /// call is taking place, and that destroy is only called once.
    ///
    /// Also, read the type-level safety section.
    #[inline]
    pub unsafe fn destroy(self) {
        unsafe {
            let r = ffi::pthread_barrier_destroy(self.as_raw_underlying());
            debug_assert_eq!(r, 0);
        }
    }

    /// Blocks until as many threads as the barrier's count, this one included, are waiting on it,
    /// then releases them all. The threads may belong to any process that maps the barrier. This
    /// is [`pthread_barrier_wait`](https://man7.org/linux/man-pages/man3/pthread_barrier_wait.3p.html).
    ///
    /// # Safety
    /// Read the type-level safety section.
    ///
    /// # Errors
    /// See [`BarrierWaitError`]
    #[inline]
    pub unsafe fn wait(&self) -> Result<BarrierWaitOutcome, BarrierWaitError> {
        wait_on(self.as_raw_underlying())
    }

    /// Returns a pointer to the underlying `pthread_barrier_t`.
    #[inline]
    pub fn as_raw_barrier(&self) -> *mut pthread_barrier_t {
        self.as_raw_underlying()
    }
}
