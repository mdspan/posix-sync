//! RAII guards returned from successful lock operations.

use std::fmt::{self, Debug};
use std::marker::PhantomData;

use libc::pthread_mutex_t;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use super::errors::MutexLockError;
use crate::utils::{AsRawUnderlying, Sealed};

/// Implemented by every guard in this module.
///
/// It exists so that a condition variable can be handed any of them without caring which lock
/// operation produced it. This trait is sealed.
pub trait MutexGuard: Sealed + AsRawUnderlying<Underlying = pthread_mutex_t> {
    /// Arranges for the data protected by the mutex to be marked consistent again. Which of the
    /// two moments that happens at depends on the guard: [`StandardGuard`] does it at once, while
    /// an indeterminate guard records the request and acts on it when it is dropped.
    ///
    /// This is only meaningful for a robust mutex whose previous owner died while holding the
    /// lock. Every other mutex reports [`MutexLockError::Invalid`].
    ///
    /// # Errors
    /// See [`MutexLockError`]
    #[cfg_attr(docsrs, doc(cfg(any(target_os = "linux", target_os = "freebsd"))))]
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn mark_consistent(&mut self) -> Result<(), MutexLockError>;
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod robust;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub use robust::*;

/// A RAII guard returned by a typical lock operation.
///
/// Namely, it can be returned in the following two cases:
/// - After a successful lock on a standard mutex.
/// - As a variant of `RobustGuardContainer` after a successful lock on a robust mutex.
pub struct StandardGuard<'a> {
    pub(super) raw: *mut pthread_mutex_t,
    pub(super) _phantom: PhantomData<&'a ()>,
}

unsafe impl Sync for StandardGuard<'_> {}
impl Sealed for StandardGuard<'_> {}

impl AsRawUnderlying for StandardGuard<'_> {
    type Underlying = pthread_mutex_t;

    fn as_raw_underlying(&self) -> *mut pthread_mutex_t {
        self.raw
    }
}

impl MutexGuard for StandardGuard<'_> {
    /// Calls `pthread_mutex_consistent` immediately, since a standard guard has no deferred
    /// consistency state to record the request in.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[inline]
    fn mark_consistent(&mut self) -> Result<(), MutexLockError> {
        match unsafe { libc::pthread_mutex_consistent(self.raw) } {
            0 => Ok(()),
            e => Err(MutexLockError::from(e)),
        }
    }
}

impl Debug for StandardGuard<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StandardGuard").finish_non_exhaustive()
    }
}

impl StandardGuard<'_> {
    /// Constructs the guard from a mutex that has just been locked.
    pub(super) fn from_mutex<T>(mtx: &T) -> StandardGuard<'_>
    where
        T: AsRawUnderlying<Underlying = pthread_mutex_t>,
    {
        StandardGuard {
            raw: mtx.as_raw_underlying(),
            _phantom: PhantomData,
        }
    }
}

impl Drop for StandardGuard<'_> {
    /// Unlocks the associated mutex.
    #[inline]
    fn drop(&mut self) {
        unsafe {
            let r = libc::pthread_mutex_unlock(self.raw);
            debug_assert_eq!(r, 0);
        }
    }
}
