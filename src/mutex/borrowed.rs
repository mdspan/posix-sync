use std::cell::UnsafeCell;
use std::fmt::{self, Debug};
use std::marker::{PhantomData, Send, Sync, Unpin};

use libc::pthread_mutex_t;

use super::errors::MutexLockError;
use super::robustness_markers::RobustnessMarker;
use super::{AsRawUnderlying, RawMutexAlloc};
use crate::utils::Sealed;

/// A mutex that borrows the memory for its underlying mutex. The underlying mutex will **not** be
/// destroyed when `BorrowedMutex` is dropped. It is the caller's responsibility to call
/// [`destroy`](BorrowedMutex::destroy) when the mutex is no longer needed.
///
/// # Safety
///
/// The methods of `BorrowedMutex`, unlike those of [`OwnedMutex`](crate::mutex::OwnedMutex), are
/// unsafe. This is because the underlying mutex may be in a shared memory mapping, where it may
/// be modifiable by other processes, and calling these methods can lead to undefined behaviour if
/// certain invariants are not upheld. Here is a non-exhaustive list of some of the situations that
/// will lead to undefined behaviour:
///
/// - Another process destroys the mutex before your process calls `destroy`. Calling any method on
///   the mutex (even destroy itself) is now UB.
/// - Another process unlocks the mutex without holding a lock.
/// - A process owns a lock and tries to lock the mutex again (unless the mutex is error-checked).
///
/// For a more thorough description of what situations can result in UB, read some of the `pthread_mutex_*`
/// pages in the [POSIX standard](https://pubs.opengroup.org/onlinepubs/9799919799/idx/ip.html).
pub struct BorrowedMutex<'a, R: RobustnessMarker> {
    raw: *mut RawMutexAlloc,

    /// The `*const UnsafeCell` is to prevent the type from being `UnwindSafe` and `RefUnwindSafe`.
    _phantom: PhantomData<(*const UnsafeCell<R>, &'a ())>,
}

impl<R: RobustnessMarker> Sealed for BorrowedMutex<'_, R> {}
unsafe impl<R: RobustnessMarker> Send for BorrowedMutex<'_, R> {}
unsafe impl<R: RobustnessMarker> Sync for BorrowedMutex<'_, R> {}
impl<R: RobustnessMarker> Unpin for BorrowedMutex<'_, R> {}

impl<R: RobustnessMarker> Clone for BorrowedMutex<'_, R> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<R: RobustnessMarker> Copy for BorrowedMutex<'_, R> {}

impl<R: RobustnessMarker> AsRawUnderlying for BorrowedMutex<'_, R> {
    type Underlying = pthread_mutex_t;

    fn as_raw_underlying(&self) -> *mut pthread_mutex_t {
        self.raw as *mut _
    }
}

impl<R: RobustnessMarker> Debug for BorrowedMutex<'_, R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BorrowedMutex")
            .field("raw", &self.raw)
            .finish()
    }
}

impl<R: RobustnessMarker> BorrowedMutex<'_, R> {
    /// Constructs a `BorrowedMutex` from an existing, initialised underlying mutex at a given
    /// location in memory.
    ///
    /// - `raw`: A pointer to an existing raw mutex. Note that, unlike
    ///   [`MutexBuilder::build_borrowed`](crate::mutex::MutexBuilder::build_borrowed),
    ///   this method expects that the pointee is initialised.
    ///
    /// - `_memory`: A reference to a RAII object whose lifetime determines the validity of
    ///   `raw` (e.g. a struct that manages a memory map).
    ///
    /// # Safety
    /// The caller must guarantee that `raw` points to a valid, initialised [`RawMutexAlloc`]
    /// (a.k.a. `pthread_mutex_t`) whose robustness attribute matches `R`. Getting the robustness
    /// wrong does not corrupt anything by itself, but it does mean the guard type is lying about
    /// which outcomes are possible.
    ///
    /// Also, read the type-level safety section.
    #[inline]
    pub unsafe fn from_raw<T>(raw: *mut RawMutexAlloc, _memory: &T) -> BorrowedMutex<'_, R> {
        BorrowedMutex {
            raw,
            _phantom: PhantomData,
        }
    }

    /// Calls [`pthread_mutex_destroy`](https://man7.org/linux/man-pages/man3/pthread_mutex_destroy.3p.html)
    /// on the underlying mutex. It is safe to re-initialise another mutex at this address
    /// afterwards.
    ///
    /// Failure to call this function before destroying the mutex can result in resource leaks,
    /// but will not lead to undefined behaviour.
    ///
    /// # Safety
    /// The caller must ensure that there is no way for the mutex
    /// to be locked by any thread (from any process) while the call is taking place. Notably, this
    /// means the mutex can not be in the middle of a wait call to a condvar.
    ///
    /// Moreover, the caller must ensure that destroy is only called once.
    ///
    /// Attempting to use the mutex after it has been destroyed or destroying the same mutex twice
    /// is undefined behaviour. You can, however, safely create a new mutex at the same address
    /// after destroying the old one.
    ///
    /// Also, read the type-level safety section.
    #[inline]
    pub unsafe fn destroy(self) {
        unsafe {
            let r = libc::pthread_mutex_destroy(self.as_raw_underlying());
            debug_assert_eq!(r, 0);
        }
    }

    /// Attempts to lock the mutex in a non-blocking manner. If a lock was obtained, the `Some`
    /// variant is returned. If a lock could not be obtained but otherwise no error occurred, the
    /// `None` variant is returned. Otherwise, an error is returned.
    ///
    /// # Safety
    /// This method is unsafe because the backing mutex may be in shared memory and modifiable by
    /// many processes, where it can potentially be left in an invalid state. As long as processes
    /// only modify the mutex through libc's `pthread*` functions and don't cause any undefined
    /// behaviour on their own such as double unlocking or double locking (unless the mutex is
    /// recursive), it should generally be safe to call this function.
    ///
    /// Also, read the type-level safety section.
    ///
    /// # Errors
    /// See [`MutexLockError`]
    #[inline]
    pub unsafe fn try_lock(&self) -> Result<Option<R::Guard<'_>>, MutexLockError> {
        match libc::pthread_mutex_trylock(self.as_raw_underlying()) {
            libc::EBUSY => Ok(None),
            e => R::guard_from_libc_returnval(self, e).map(Some),
        }
    }

    /// Attempts to lock the mutex, blocking until a lock is obtained.
    ///
    /// # Safety
    /// This method is unsafe because the backing mutex may be in shared memory and modifiable by
    /// many processes, where it can potentially be left in an invalid state. As long as processes
    /// only modify the mutex through libc's `pthread*` functions and don't cause any undefined
    /// behaviour on their own such as double unlocking or double locking (unless the mutex is
    /// recursive), it should generally be safe to call this function.
    ///
    /// Also, read the type-level safety section.
    ///
    /// # Errors
    /// See [`MutexLockError`]
    #[inline]
    pub unsafe fn lock(&self) -> Result<R::Guard<'_>, MutexLockError> {
        R::guard_from_libc_returnval(self, libc::pthread_mutex_lock(self.as_raw_underlying()))
    }

    /// Returns a pointer to the underlying `pthread_mutex_t`.
    #[inline]
    pub fn as_raw_mutex(&self) -> *mut pthread_mutex_t {
        self.as_raw_underlying()
    }
}
