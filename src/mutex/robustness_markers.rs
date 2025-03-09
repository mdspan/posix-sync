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
/// Robustness is a type parameter rather than a runtime attribute so that the guard type can
/// depend on it. A robust mutex hands back a `RobustGuardContainer`, which has to be matched on
/// before it is of any use, while a standard mutex hands back a plain [`StandardGuard`]. Neither
/// kind of lock can be used as if it were the other by accident.
///
/// [`Standard`] is the only marker on the platforms without robust mutexes, so this trait has
/// exactly one implementor there. This trait is sealed.
pub trait RobustnessMarker: Clone + Debug + Sealed {
    /// The associated libc constant. Only present where the platform has robust mutexes at all;
    /// everywhere else there is no robustness attribute to set.
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
/// If the owner of a robust mutex terminates without unlocking it, the next thread to lock it
/// receives an [`Indeterminate`](RobustGuardContainer::Indeterminate) guard rather than blocking
/// forever. That guard is the only opportunity to repair whatever the dead owner left behind:
/// unless it is marked consistent, every subsequent lock fails with
/// [`NotRecoverable`](MutexLockError::NotRecoverable).
///
/// POSIX defines the attribute and the `EOWNERDEAD` notification in
/// [`pthread_mutexattr_setrobust`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_mutexattr_setrobust.html),
/// and the recovery it obliges you to perform in
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
/// If the owner of a stalled mutex terminates without unlocking it, everyone waiting on it stays
/// blocked. This is what `std::sync::Mutex` does, minus the poisoning.
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
