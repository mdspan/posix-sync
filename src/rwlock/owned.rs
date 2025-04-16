use std::cell::UnsafeCell;
use std::fmt::{self, Debug};
use std::marker::{PhantomData, Unpin};
use std::mem::MaybeUninit;
#[cfg(not(target_vendor = "apple"))]
use std::time::Duration;

use libc::pthread_rwlock_t;

use super::builders::RwLockBuilder;
use super::errors::RwLockError;
use super::guards::{
    read_guard_from_libc_returnval, write_guard_from_libc_returnval, ReadGuard, WriteGuard,
};
use super::{AsRawUnderlying, RawRwLockAlloc};
#[cfg(not(target_vendor = "apple"))]
use crate::ffi;
#[cfg(not(target_vendor = "apple"))]
use crate::utils::deadline_from_now;
use crate::utils::Sealed;

/// A rwlock that owns its underlying raw rwlock. Dropping the `OwnedRwLock` destroys it, provided
/// nobody still holds it: destroying a held rwlock is undefined, so a guard that was leaked rather
/// than dropped leaks the pthread object with it. If you want to construct a rwlock in shared
/// memory for IPC, use a [`BorrowedRwLock`](crate::rwlock::BorrowedRwLock) instead.
///
/// Because the allocation belongs to this object and nothing outside the process can reach it, the
/// locking methods are safe.
pub struct OwnedRwLock {
    raw: HeapRawRwLock,

    /// The `*const UnsafeCell` is to prevent the type from being `UnwindSafe` and `RefUnwindSafe`.
    _phantom: PhantomData<*const UnsafeCell<()>>,
}

impl Sealed for OwnedRwLock {}
unsafe impl Send for OwnedRwLock {}
unsafe impl Sync for OwnedRwLock {}
impl Unpin for OwnedRwLock {}

impl AsRawUnderlying for OwnedRwLock {
    type Underlying = pthread_rwlock_t;

    fn as_raw_underlying(&self) -> *mut pthread_rwlock_t {
        self.raw.as_raw_underlying()
    }
}

impl Debug for OwnedRwLock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnedRwLock").finish_non_exhaustive()
    }
}

impl Default for OwnedRwLock {
    /// Equivalent to `RwLockBuilder::new().build_owned()`.
    fn default() -> Self {
        RwLockBuilder::new().build_owned()
    }
}

impl Drop for OwnedRwLock {
    /// Calls [`pthread_rwlock_destroy`](https://man7.org/linux/man-pages/man3/pthread_rwlock_destroy.3p.html)
    /// on the underlying rwlock if nobody holds it.
    #[inline]
    fn drop(&mut self) {
        let raw = self.as_raw_underlying();

        // Destroying a held rwlock is undefined, and a guard that was leaked rather than dropped
        // leaves one held. Taking the write lock is the only portable way to ask whether anyone
        // still holds it, since it is the one that excludes readers too.
        unsafe {
            if libc::pthread_rwlock_trywrlock(raw) != 0 {
                return;
            }
            let r = libc::pthread_rwlock_unlock(raw);
            debug_assert_eq!(r, 0);
            let r = libc::pthread_rwlock_destroy(raw);
            debug_assert_eq!(r, 0);
        }
    }
}

impl OwnedRwLock {
    /// Creates a rwlock with the default attributes. Use [`RwLockBuilder`] to set any of the
    /// others.
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates the storage without initialising it. Only [`RwLockBuilder::build_owned`], which
    /// initialises it immediately afterwards, may call this.
    #[inline]
    pub(super) fn new_uninit() -> Self {
        Self {
            raw: HeapRawRwLock::new(),
            _phantom: PhantomData,
        }
    }

    /// Acquires a read lock, blocking until it is available. Any number of readers can hold the
    /// lock at the same time.
    ///
    /// # Errors
    /// See [`RwLockError`]
    #[inline]
    pub fn read(&self) -> Result<ReadGuard<'_>, RwLockError> {
        let r = unsafe { libc::pthread_rwlock_rdlock(self.as_raw_underlying()) };
        read_guard_from_libc_returnval(self, r)
    }

    /// Attempts to acquire a read lock without blocking. A lock that a writer is holding, or that
    /// a writer is waiting for on a writer-preferring rwlock, is reported as `Ok(None)`.
    ///
    /// # Errors
    /// See [`RwLockError`]
    #[inline]
    pub fn try_read(&self) -> Result<Option<ReadGuard<'_>>, RwLockError> {
        match unsafe { libc::pthread_rwlock_tryrdlock(self.as_raw_underlying()) } {
            libc::EBUSY => Ok(None),
            e => read_guard_from_libc_returnval(self, e).map(Some),
        }
    }

    /// Acquires a read lock, blocking until it is available or `timeout` elapses. A lock that
    /// could not be acquired in time is reported as `Ok(None)`.
    ///
    /// The timeout is resolved against `CLOCK_REALTIME`, which is the clock
    /// [`pthread_rwlock_timedrdlock`](https://man7.org/linux/man-pages/man3/pthread_rwlock_timedrdlock.3p.html)
    /// is defined in terms of. Stepping the system clock therefore moves the deadline.
    ///
    /// Not available on Apple platforms, which do not implement the timed rwlock
    /// functions.
    ///
    /// # Errors
    /// See [`RwLockError`]
    #[cfg_attr(docsrs, doc(cfg(not(target_vendor = "apple"))))]
    #[cfg(not(target_vendor = "apple"))]
    #[inline]
    pub fn read_for(&self, timeout: Duration) -> Result<Option<ReadGuard<'_>>, RwLockError> {
        let deadline = deadline_from_now(libc::CLOCK_REALTIME, timeout);
        match unsafe { ffi::pthread_rwlock_timedrdlock(self.as_raw_underlying(), &deadline) } {
            libc::ETIMEDOUT => Ok(None),
            e => read_guard_from_libc_returnval(self, e).map(Some),
        }
    }

    /// Acquires the write lock, blocking until it is available. Nobody else holds the lock while
    /// the returned guard is alive.
    ///
    /// # Errors
    /// See [`RwLockError`]
    #[inline]
    pub fn write(&self) -> Result<WriteGuard<'_>, RwLockError> {
        let r = unsafe { libc::pthread_rwlock_wrlock(self.as_raw_underlying()) };
        write_guard_from_libc_returnval(self, r)
    }

    /// Attempts to acquire the write lock without blocking. A lock anyone else holds, reader or
    /// writer, is reported as `Ok(None)`.
    ///
    /// # Errors
    /// See [`RwLockError`]
    #[inline]
    pub fn try_write(&self) -> Result<Option<WriteGuard<'_>>, RwLockError> {
        match unsafe { libc::pthread_rwlock_trywrlock(self.as_raw_underlying()) } {
            libc::EBUSY => Ok(None),
            e => write_guard_from_libc_returnval(self, e).map(Some),
        }
    }

    /// Acquires the write lock, blocking until it is available or `timeout` elapses. A lock that
    /// could not be acquired in time is reported as `Ok(None)`.
    ///
    /// The timeout is resolved against `CLOCK_REALTIME`, as for [`read_for`](Self::read_for).
    ///
    /// Not available on Apple platforms, which do not implement the timed rwlock
    /// functions.
    ///
    /// # Errors
    /// See [`RwLockError`]
    #[cfg_attr(docsrs, doc(cfg(not(target_vendor = "apple"))))]
    #[cfg(not(target_vendor = "apple"))]
    #[inline]
    pub fn write_for(&self, timeout: Duration) -> Result<Option<WriteGuard<'_>>, RwLockError> {
        let deadline = deadline_from_now(libc::CLOCK_REALTIME, timeout);
        match unsafe { ffi::pthread_rwlock_timedwrlock(self.as_raw_underlying(), &deadline) } {
            libc::ETIMEDOUT => Ok(None),
            e => write_guard_from_libc_returnval(self, e).map(Some),
        }
    }

    /// Returns a pointer to the underlying `pthread_rwlock_t`.
    ///
    /// The pointer stays valid until this `OwnedRwLock` is dropped.
    #[inline]
    pub fn as_raw_rwlock(&self) -> *mut pthread_rwlock_t {
        self.as_raw_underlying()
    }
}

/// A heap-allocated raw rwlock allocation.
struct HeapRawRwLock(*mut MaybeUninit<RawRwLockAlloc>);

impl Sealed for HeapRawRwLock {}

impl AsRawUnderlying for HeapRawRwLock {
    type Underlying = pthread_rwlock_t;

    fn as_raw_underlying(&self) -> *mut pthread_rwlock_t {
        self.0 as *mut _
    }
}

impl HeapRawRwLock {
    fn new() -> Self {
        let boxed_uninit = Box::new(MaybeUninit::uninit());
        let ptr = Box::into_raw(boxed_uninit);
        Self(ptr)
    }
}

impl Drop for HeapRawRwLock {
    fn drop(&mut self) {
        // # Safety
        // This is fine because the pointer came from `Box::into_raw`.
        unsafe {
            let _ = Box::from_raw(self.0);
        }
    }
}
