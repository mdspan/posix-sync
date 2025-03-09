use std::cell::UnsafeCell;
use std::fmt::{self, Debug};
use std::marker::{PhantomData, Unpin};
use std::mem::MaybeUninit;

use libc::pthread_mutex_t;

use super::builders::MutexBuilder;
use super::errors::MutexLockError;
use super::robustness_markers::RobustnessMarker;
use super::{AsRawUnderlying, RawMutexAlloc};
use crate::utils::Sealed;

/// A mutex that owns its underlying raw mutex. Dropping the `OwnedMutex` destroys it, provided
/// nobody still holds the lock: destroying a locked mutex is undefined, so a guard that was leaked
/// rather than dropped leaks the pthread object with it. If you want to construct a mutex in
/// shared memory for IPC, use a [`BorrowedMutex`](crate::mutex::BorrowedMutex) instead.
///
/// Because the allocation belongs to this object and nothing outside the process can reach it,
/// the locking methods are safe.
pub struct OwnedMutex<R: RobustnessMarker> {
    raw: HeapRawMutex,

    /// The `*const UnsafeCell` is to prevent the type from being `UnwindSafe` and `RefUnwindSafe`.
    _phantom: PhantomData<*const UnsafeCell<R>>,
}

impl<R: RobustnessMarker> Sealed for OwnedMutex<R> {}
unsafe impl<R: RobustnessMarker> Send for OwnedMutex<R> {}
unsafe impl<R: RobustnessMarker> Sync for OwnedMutex<R> {}
impl<R: RobustnessMarker> Unpin for OwnedMutex<R> {}

impl<R: RobustnessMarker> AsRawUnderlying for OwnedMutex<R> {
    type Underlying = pthread_mutex_t;

    fn as_raw_underlying(&self) -> *mut pthread_mutex_t {
        self.raw.as_raw_underlying()
    }
}

impl<R: RobustnessMarker> Debug for OwnedMutex<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedMutex").finish_non_exhaustive()
    }
}

impl<R: RobustnessMarker> Default for OwnedMutex<R> {
    /// Equivalent to `MutexBuilder::<R>::new().build_owned()`.
    fn default() -> Self {
        MutexBuilder::<R>::new().build_owned()
    }
}

impl<R: RobustnessMarker> Drop for OwnedMutex<R> {
    /// Calls [`pthread_mutex_destroy`](https://man7.org/linux/man-pages/man3/pthread_mutex_destroy.3p.html)
    /// on the underlying mutex if it is unlocked.
    #[inline]
    fn drop(&mut self) {
        let raw = self.as_raw_underlying();

        // Destroying a locked mutex is undefined, and a guard that was leaked rather than dropped
        // leaves one locked. Taking the lock is the only portable way to ask whether anyone still
        // owns it.
        unsafe {
            match libc::pthread_mutex_trylock(raw) {
                0 | libc::EOWNERDEAD => {
                    let r = libc::pthread_mutex_unlock(raw);
                    debug_assert_eq!(r, 0);
                }
                // A robust mutex nobody can lock again has no owner left to disturb either.
                libc::ENOTRECOVERABLE => {}
                _ => return,
            }
            // Refuses with EBUSY if a recursive lock is still outstanding, in which case the
            // pthread object is leaked along with the guard that was never dropped.
            libc::pthread_mutex_destroy(raw);
        }
    }
}

impl<R: RobustnessMarker> OwnedMutex<R> {
    /// Creates a mutex with the default attributes for its robustness marker. Use
    /// [`MutexBuilder`] to set any of the others.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates the storage without initialising it. Only [`MutexBuilder::build_owned`], which
    /// initialises it immediately afterwards, may call this.
    #[inline]
    pub(super) fn new_uninit() -> Self {
        Self {
            raw: HeapRawMutex::new(),
            _phantom: PhantomData,
        }
    }

    /// Attempts to lock the mutex in a non-blocking manner. If a lock was obtained, the `Some`
    /// variant is returned. If a lock could not be obtained but otherwise no error occurred, the
    /// `None` variant is returned. Otherwise, an error is returned.
    ///
    /// # Errors
    /// See [`MutexLockError`]
    #[inline]
    pub fn try_lock(&self) -> Result<Option<R::Guard<'_>>, MutexLockError> {
        match unsafe { libc::pthread_mutex_trylock(self.as_raw_underlying()) } {
            libc::EBUSY => Ok(None),
            e => R::guard_from_libc_returnval(self, e).map(Some),
        }
    }

    /// Attempts to lock the mutex, blocking until a lock is obtained.
    ///
    /// # Errors
    /// See [`MutexLockError`]
    #[inline]
    pub fn lock(&self) -> Result<R::Guard<'_>, MutexLockError> {
        let r = unsafe { libc::pthread_mutex_lock(self.as_raw_underlying()) };
        R::guard_from_libc_returnval(self, r)
    }

    /// Returns a pointer to the underlying `pthread_mutex_t`.
    ///
    /// The pointer stays valid until this `OwnedMutex` is dropped.
    #[inline]
    pub fn as_raw_mutex(&self) -> *mut pthread_mutex_t {
        self.as_raw_underlying()
    }
}

/// A heap-allocated raw mutex allocation.
struct HeapRawMutex(*mut MaybeUninit<RawMutexAlloc>);

impl Sealed for HeapRawMutex {}

impl AsRawUnderlying for HeapRawMutex {
    type Underlying = pthread_mutex_t;

    fn as_raw_underlying(&self) -> *mut pthread_mutex_t {
        self.0 as *mut _
    }
}

impl HeapRawMutex {
    fn new() -> Self {
        let boxed_uninit = Box::new(MaybeUninit::uninit());
        let ptr = Box::into_raw(boxed_uninit);
        Self(ptr)
    }
}

impl Drop for HeapRawMutex {
    fn drop(&mut self) {
        // # Safety
        // This is fine because the pointer came from `Box::into_raw`.
        unsafe {
            let _ = Box::from_raw(self.0);
        }
    }
}
