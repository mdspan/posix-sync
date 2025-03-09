//! The guards a robust mutex can hand out, and the consistency bookkeeping that goes with them.
//!
//! Only compiled where the platform actually has robust mutexes. See the
//! [`robustness_markers`](super::super::robustness_markers) module for the list.

use std::fmt::{self, Debug};
use std::marker::PhantomData;
use std::mem::ManuallyDrop;

use libc::pthread_mutex_t;

use super::{MutexGuard, StandardGuard};
use crate::mutex::errors::MutexLockError;
use crate::utils::{AsRawUnderlying, Sealed};

#[cfg_attr(docsrs, doc(cfg(any(target_os = "linux", target_os = "freebsd"))))]
/// Specifies the state the data protected by the robust mutex is in.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DataConsistency {
    /// Use this variant if you can guarantee that the data protected by the mutex is valid.
    Consistent,
    /// Use this variant if you can not guarantee that the data protected by the mutex is
    /// valid. This is the state the guard is in by default when it is returned from a lock.
    Irrecoverable,
}

#[cfg_attr(docsrs, doc(cfg(any(target_os = "linux", target_os = "freebsd"))))]
/// A RAII guard returned as a variant of [`RobustGuardContainer`] by a robust mutex when
/// the last owner terminated before unlocking.
pub struct IndeterminateGuard<'a> {
    pub(super) raw: *mut pthread_mutex_t,
    pub(super) consistency: DataConsistency,
    pub(super) _phantom: PhantomData<&'a ()>,
}

// core trait implementations
unsafe impl Sync for IndeterminateGuard<'_> {}
impl Sealed for IndeterminateGuard<'_> {}

impl AsRawUnderlying for IndeterminateGuard<'_> {
    type Underlying = pthread_mutex_t;

    fn as_raw_underlying(&self) -> *mut pthread_mutex_t {
        self.raw
    }
}

impl MutexGuard for IndeterminateGuard<'_> {
    /// Equivalent to `set_consistency_on_unlock(DataConsistency::Consistent)`, which cannot fail.
    #[inline]
    fn mark_consistent(&mut self) -> Result<(), MutexLockError> {
        self.set_consistency_on_unlock(DataConsistency::Consistent);
        Ok(())
    }
}

impl Debug for IndeterminateGuard<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IndeterminateGuard")
            .field("consistency", &self.consistency)
            .finish_non_exhaustive()
    }
}

impl<'a> IndeterminateGuard<'a> {
    /// Constructs the guard from a mutex that has just been locked.
    pub(in crate::mutex) fn from_mutex<T>(mtx: &T) -> IndeterminateGuard<'_>
    where
        T: AsRawUnderlying<Underlying = pthread_mutex_t>,
    {
        IndeterminateGuard {
            raw: mtx.as_raw_underlying(),
            consistency: DataConsistency::Irrecoverable,
            _phantom: PhantomData,
        }
    }

    /// Sets the consistency state the mutex will be in after unlock. By default, the guard
    /// assumes that the consistency is [`Irrecoverable`](DataConsistency::Irrecoverable).
    /// If the consistency is set to [`Consistent`][`DataConsistency::Consistent`], then
    /// [`pthread_mutex_consistent`](https://man7.org/linux/man-pages/man3/pthread_mutex_consistent.3.html)
    /// will be called on the underlying mutex when the guard is dropped. You should only set
    /// this consistency state if you are certain that the data protected by the mutex is
    /// in a valid state.
    ///
    /// In the case that the mutex is not made consistent, the next lock will error with
    /// [`MutexLockError::NotRecoverable`].
    pub fn set_consistency_on_unlock(&mut self, consistency: DataConsistency) {
        self.consistency = consistency;
    }

    /// The consistency state the mutex will be left in when this guard is dropped.
    pub fn consistency_on_unlock(&self) -> DataConsistency {
        self.consistency
    }

    /// Marks the mutex consistent right away and hands back a plain guard, keeping the lock held
    /// the whole time.
    ///
    /// This is the same thing [`set_consistency_on_unlock`](Self::set_consistency_on_unlock) does
    /// on drop, done eagerly so that the rest of the critical section no longer has to carry the
    /// distinction around. If the call fails the lock is released, since a guard that could
    /// neither be repaired nor converted has nothing left to hold onto.
    ///
    /// # Errors
    /// See [`MutexLockError`]
    pub fn make_consistent(self) -> Result<StandardGuard<'a>, MutexLockError> {
        let raw = self.raw;
        let this = ManuallyDrop::new(self);

        match unsafe { libc::pthread_mutex_consistent(raw) } {
            0 => Ok(StandardGuard {
                raw,
                _phantom: PhantomData,
            }),
            e => {
                drop(ManuallyDrop::into_inner(this));
                Err(MutexLockError::from(e))
            }
        }
    }
}

impl Drop for IndeterminateGuard<'_> {
    /// Unlocks the associated mutex, optionally marking it consistent.
    #[inline]
    fn drop(&mut self) {
        unsafe {
            if let DataConsistency::Consistent = self.consistency {
                let r = libc::pthread_mutex_consistent(self.raw);
                debug_assert_eq!(r, 0);
            };
            let r = libc::pthread_mutex_unlock(self.raw);
            debug_assert_eq!(r, 0);
        }
    }
}

#[cfg_attr(docsrs, doc(cfg(any(target_os = "linux", target_os = "freebsd"))))]
/// The RAII guard returned by robust mutexes.
#[derive(Debug)]
pub enum RobustGuardContainer<'a> {
    /// The last owner unlocked the mutex as usual.
    Standard(StandardGuard<'a>),
    /// The last owner terminated before they could unlock the mutex. Note that this is not the
    /// same as the last owner panicking, as in those cases the guard is dropped when unwinding,
    /// unlocking the mutex (unless the last owner panicked twice before unlocking).
    Indeterminate(IndeterminateGuard<'a>),
}

impl Sealed for RobustGuardContainer<'_> {}

impl AsRawUnderlying for RobustGuardContainer<'_> {
    type Underlying = pthread_mutex_t;

    fn as_raw_underlying(&self) -> *mut pthread_mutex_t {
        match self {
            Self::Standard(g) => g.as_raw_underlying(),
            Self::Indeterminate(g) => g.as_raw_underlying(),
        }
    }
}

impl MutexGuard for RobustGuardContainer<'_> {
    #[inline]
    fn mark_consistent(&mut self) -> Result<(), MutexLockError> {
        match self {
            Self::Standard(g) => g.mark_consistent(),
            Self::Indeterminate(g) => g.mark_consistent(),
        }
    }
}

impl<'a> RobustGuardContainer<'a> {
    /// Returns `true` if the previous owner of the mutex died while holding the lock, leaving the
    /// protected data in an unknown state.
    pub fn is_indeterminate(&self) -> bool {
        matches!(self, Self::Indeterminate(_))
    }

    /// Collapses the container into a plain guard, marking the protected data consistent if the
    /// previous owner died holding the lock.
    ///
    /// Only call this once whatever the dead owner left behind has been repaired: it is the point
    /// at which the mutex stops reporting [`MutexLockError::NotRecoverable`] to everyone else.
    ///
    /// # Errors
    /// See [`MutexLockError`]
    pub fn make_consistent(self) -> Result<StandardGuard<'a>, MutexLockError> {
        match self {
            Self::Standard(g) => Ok(g),
            Self::Indeterminate(g) => g.make_consistent(),
        }
    }
}
