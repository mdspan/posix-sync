use std::cell::UnsafeCell;
use std::fmt::{self, Debug};
use std::marker::{PhantomData, Send, Sync, Unpin};
#[cfg(not(target_vendor = "apple"))]
use std::time::Duration;

use libc::pthread_rwlock_t;

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

/// A rwlock that borrows the memory for its underlying rwlock. The underlying rwlock will **not**
/// be destroyed when `BorrowedRwLock` is dropped. It is the caller's responsibility to call
/// [`destroy`](BorrowedRwLock::destroy) when the rwlock is no longer needed.
///
/// # Safety
///
/// The methods of `BorrowedRwLock`, unlike those of [`OwnedRwLock`](crate::rwlock::OwnedRwLock),
/// are unsafe. This is because the underlying rwlock may be in a shared memory mapping, where it
/// may be modifiable by other processes, and calling these methods can lead to undefined behaviour
/// if certain invariants are not upheld. Here is a non-exhaustive list of some of the situations
/// that will lead to undefined behaviour:
///
/// - Another process destroys the rwlock before your process calls `destroy`. Calling any method
///   on the rwlock (even destroy itself) is now UB.
/// - Another process unlocks the rwlock without holding it.
/// - A process holding the write lock asks for it again.
///
/// There is also no recovery from a process that dies holding the write lock: unlike a robust
/// [`mutex`](crate::mutex), POSIX gives a rwlock no way to report that its holder is gone, and
/// everyone else simply blocks forever.
///
/// For a more thorough description of what situations can result in UB, read some of the
/// `pthread_rwlock_*` pages in the
/// [POSIX standard](https://pubs.opengroup.org/onlinepubs/9799919799/idx/ip.html).
pub struct BorrowedRwLock<'a> {
    raw: *mut RawRwLockAlloc,

    /// The `*const UnsafeCell` is to prevent the type from being `UnwindSafe` and `RefUnwindSafe`.
    _phantom: PhantomData<(*const UnsafeCell<()>, &'a ())>,
}

impl Sealed for BorrowedRwLock<'_> {}
unsafe impl Send for BorrowedRwLock<'_> {}
unsafe impl Sync for BorrowedRwLock<'_> {}
impl Unpin for BorrowedRwLock<'_> {}

impl Clone for BorrowedRwLock<'_> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl Copy for BorrowedRwLock<'_> {}

impl AsRawUnderlying for BorrowedRwLock<'_> {
    type Underlying = pthread_rwlock_t;

    fn as_raw_underlying(&self) -> *mut pthread_rwlock_t {
        self.raw as *mut _
    }
}

impl Debug for BorrowedRwLock<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BorrowedRwLock")
            .field("raw", &self.raw)
            .finish()
    }
}

impl BorrowedRwLock<'_> {
    /// Constructs a `BorrowedRwLock` from an existing, initialised underlying rwlock at a given
    /// location in memory.
    ///
    /// - `raw`: A pointer to an existing raw rwlock. Note that, unlike
    ///   [`RwLockBuilder::build_borrowed`](crate::rwlock::RwLockBuilder::build_borrowed),
    ///   this method expects that the pointee is initialised.
    ///
    /// - `_memory`: A reference to a RAII object whose lifetime determines the validity of `raw`
    ///   (e.g. a struct that manages a memory map).
    ///
    /// # Safety
    /// The caller must guarantee that `raw` points to a valid, initialised [`RawRwLockAlloc`]
    /// (a.k.a. `pthread_rwlock_t`).
    ///
    /// Also, read the type-level safety section.
    #[inline]
    pub unsafe fn from_raw<T>(raw: *mut RawRwLockAlloc, _memory: &T) -> BorrowedRwLock<'_> {
        BorrowedRwLock {
            raw,
            _phantom: PhantomData,
        }
    }

    /// Calls [`pthread_rwlock_destroy`](https://man7.org/linux/man-pages/man3/pthread_rwlock_destroy.3p.html)
    /// on the underlying rwlock. It is safe to re-initialise another rwlock at this address
    /// afterwards.
    ///
    /// Failure to call this function before releasing the memory can result in resource leaks, but
    /// will not lead to undefined behaviour.
    ///
    /// # Safety
    /// The caller must ensure that there is no way for the rwlock to be held by any thread (from
    /// any process) while the call is taking place, and that destroy is only called once.
    ///
    /// Also, read the type-level safety section.
    #[inline]
    pub unsafe fn destroy(self) {
        unsafe {
            let r = libc::pthread_rwlock_destroy(self.as_raw_underlying());
            debug_assert_eq!(r, 0);
        }
    }

    /// Acquires a read lock, blocking until it is available. Any number of readers can hold the
    /// lock at the same time.
    ///
    /// # Safety
    /// Read the type-level safety section.
    ///
    /// # Errors
    /// See [`RwLockError`]
    #[inline]
    pub unsafe fn read(&self) -> Result<ReadGuard<'_>, RwLockError> {
        read_guard_from_libc_returnval(self, libc::pthread_rwlock_rdlock(self.as_raw_underlying()))
    }

    /// Attempts to acquire a read lock without blocking. A lock that a writer is holding, or that
    /// a writer is waiting for on a writer-preferring rwlock, is reported as `Ok(None)`.
    ///
    /// # Safety
    /// Read the type-level safety section.
    ///
    /// # Errors
    /// See [`RwLockError`]
    #[inline]
    pub unsafe fn try_read(&self) -> Result<Option<ReadGuard<'_>>, RwLockError> {
        match libc::pthread_rwlock_tryrdlock(self.as_raw_underlying()) {
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
    /// # Safety
    /// Read the type-level safety section.
    ///
    /// # Errors
    /// See [`RwLockError`]
    #[cfg_attr(docsrs, doc(cfg(not(target_vendor = "apple"))))]
    #[cfg(not(target_vendor = "apple"))]
    #[inline]
    pub unsafe fn read_for(&self, timeout: Duration) -> Result<Option<ReadGuard<'_>>, RwLockError> {
        let deadline = deadline_from_now(libc::CLOCK_REALTIME, timeout);
        match ffi::pthread_rwlock_timedrdlock(self.as_raw_underlying(), &deadline) {
            libc::ETIMEDOUT => Ok(None),
            e => read_guard_from_libc_returnval(self, e).map(Some),
        }
    }

    /// Acquires the write lock, blocking until it is available. Nobody else holds the lock while
    /// the returned guard is alive.
    ///
    /// # Safety
    /// Read the type-level safety section.
    ///
    /// # Errors
    /// See [`RwLockError`]
    #[inline]
    pub unsafe fn write(&self) -> Result<WriteGuard<'_>, RwLockError> {
        write_guard_from_libc_returnval(self, libc::pthread_rwlock_wrlock(self.as_raw_underlying()))
    }

    /// Attempts to acquire the write lock without blocking. A lock anyone else holds, reader or
    /// writer, is reported as `Ok(None)`.
    ///
    /// # Safety
    /// Read the type-level safety section.
    ///
    /// # Errors
    /// See [`RwLockError`]
    #[inline]
    pub unsafe fn try_write(&self) -> Result<Option<WriteGuard<'_>>, RwLockError> {
        match libc::pthread_rwlock_trywrlock(self.as_raw_underlying()) {
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
    /// # Safety
    /// Read the type-level safety section.
    ///
    /// # Errors
    /// See [`RwLockError`]
    #[cfg_attr(docsrs, doc(cfg(not(target_vendor = "apple"))))]
    #[cfg(not(target_vendor = "apple"))]
    #[inline]
    pub unsafe fn write_for(
        &self,
        timeout: Duration,
    ) -> Result<Option<WriteGuard<'_>>, RwLockError> {
        let deadline = deadline_from_now(libc::CLOCK_REALTIME, timeout);
        match ffi::pthread_rwlock_timedwrlock(self.as_raw_underlying(), &deadline) {
            libc::ETIMEDOUT => Ok(None),
            e => write_guard_from_libc_returnval(self, e).map(Some),
        }
    }

    /// Returns a pointer to the underlying `pthread_rwlock_t`.
    #[inline]
    pub fn as_raw_rwlock(&self) -> *mut pthread_rwlock_t {
        self.as_raw_underlying()
    }
}
