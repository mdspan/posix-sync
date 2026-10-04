//! Marker types used to select the robustness attribute of the mutex.

use std::fmt::Debug;

use libc::pthread_mutex_t;

use super::errors::MutexLockError;
use super::guards::StandardGuard;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use super::guards::{IndeterminateGuard, RobustGuardContainer};
use crate::utils::{AsRawUnderlying, Sealed};

/// Marker trait implemented by the robustness markers in this module.
///
/// `Robust` exists only on Linux and FreeBSD. Everywhere else [`Standard`] is the only marker,
/// so this trait has exactly one implementor there. This trait is sealed.
pub trait RobustnessMarker: Clone + Debug + Sealed {
    /// The libc constant associated with this robustness marker.
    #[doc(hidden)]
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    const VALUE: i32;

    /// The guard handed out by a successful lock on a mutex carrying this marker.
    type Guard<'a>;

    /// Turns the return value of one of the `pthread_mutex_*lock` functions into a guard.
    #[doc(hidden)]
    fn guard_from_libc_returnval<'m, M>(
        mtx: &'m M,
        r: i32,
    ) -> Result<Self::Guard<'m>, MutexLockError>
    where
        M: AsRawUnderlying<Underlying = pthread_mutex_t>;
}

#[cfg_attr(docsrs, doc(cfg(any(target_os = "linux", target_os = "freebsd"))))]
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
/// Marker type specifying a robust mutex.
///
/// The motivation behind robust mutexes is crash-tolerance.
/// If the owner of a robust mutex terminates without unlocking it, the next thread to lock it
/// receives an [`Indeterminate`](RobustGuardContainer::Indeterminate) guard.
/// That guard gives the thread an opportunity to repair anything the dead owner may have left
/// in an invalid state. Unless the guard is marked consistent, with
/// [`make_consistent`](IndeterminateGuard::make_consistent) or
/// [`set_consistency_on_unlock`](IndeterminateGuard::set_consistency_on_unlock), every
/// subsequent lock fails with [`NotRecoverable`](MutexLockError::NotRecoverable).
///
/// See
/// [`pthread_mutexattr_setrobust`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_mutexattr_setrobust.html),
/// and
/// [`pthread_mutex_consistent`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_mutex_consistent.html).
#[derive(Debug, Copy, Clone)]
pub struct Robust {}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl Sealed for Robust {}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
impl RobustnessMarker for Robust {
    const VALUE: i32 = libc::PTHREAD_MUTEX_ROBUST;

    type Guard<'a> = RobustGuardContainer<'a>;

    fn guard_from_libc_returnval<'m, M>(
        mtx: &'m M,
        r: i32,
    ) -> Result<RobustGuardContainer<'m>, MutexLockError>
    where
        M: AsRawUnderlying<Underlying = pthread_mutex_t>,
    {
        match r {
            0 => Ok(RobustGuardContainer::Standard(StandardGuard::from_mutex(
                mtx,
            ))),
            libc::EOWNERDEAD => Ok(RobustGuardContainer::Indeterminate(
                IndeterminateGuard::from_mutex(mtx),
            )),
            e => Err(MutexLockError::from(e)),
        }
    }
}

/// Marker type specifying a standard (a.k.a. stalled) mutex.
///
/// If the owner of a standard mutex terminates without unlocking it, other threads
/// waiting on it remain blocked. This is the behavior of `std::sync::Mutex` (minus the poisoning).
#[derive(Debug, Copy, Clone)]
pub struct Standard {}

impl Sealed for Standard {}

impl RobustnessMarker for Standard {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    const VALUE: i32 = libc::PTHREAD_MUTEX_STALLED;

    type Guard<'a> = StandardGuard<'a>;

    fn guard_from_libc_returnval<'m, M>(
        mtx: &'m M,
        r: i32,
    ) -> Result<StandardGuard<'m>, MutexLockError>
    where
        M: AsRawUnderlying<Underlying = pthread_mutex_t>,
    {
        match r {
            0 => Ok(StandardGuard::from_mutex(mtx)),
            e => Err(MutexLockError::from(e)),
        }
    }
}
