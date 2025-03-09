use std::error::Error;
use std::fmt::{Debug, Display};

use crate::utils::{impl_libc_error_conversions, write_errcode};

/// The error type returned on failed mutex locks.
///
/// The variant descriptions and `Display` impls for
/// this error type are based on info from the POSIX man pages. The semantics, however, are
/// platform-dependent. Consult the man pages of your operating system for more relevant error
/// descriptions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MutexLockError {
    /// This error can occur for several reasons:
    ///
    /// - The mutex was created with the protocol attribute
    ///   `PTHREAD_PRIO_PROTECT` and the calling thread's priority is higher than
    ///   the mutex's current priority ceiling.
    /// - The value specified by the mutex does not refer to an initialised mutex object. POSIX
    ///   recommends rather than requires reporting this, so whether it is detected is up to the
    ///   platform. It should not occur under normal circumstances.
    /// - `mark_consistent` was called through a guard whose mutex is not a robust mutex left
    ///   behind by a dead owner.
    Invalid,
    /// The calling thread does not own the mutex. Locking never reports this, because unlocking
    /// is what the underlying `EPERM` describes and the guards here unlock on drop rather than
    /// through a fallible method. It reaches callers only from a raw errno converted by hand.
    Permission,
    /// Locking would have resulted in a deadlock. This variant is typically exclusive to
    /// error-checked mutexes, but some platforms return it for
    /// [`Normal`](super::builders::MutexType::Normal) mutexes as well.
    Deadlock,
    /// The data protected by the mutex is in an inconsistent state which can not be recovered
    /// from, because the owner that died holding the lock was never followed by anyone marking
    /// the data consistent again. This variant is exclusive to robust mutexes.
    NotRecoverable,
    /// The mutex is recursive and the maximum number of locks has been exceeded.
    Again,
    /// The deadline of a timed lock passed before the lock could be acquired. The timed locking
    /// methods here report that outcome as `Ok(None)`, so this variant only shows up when
    /// a raw errno is converted by hand.
    TimedOut,
    /// None of the above. Contains the libc error code.
    Other(i32),
}

impl_libc_error_conversions! {
    [MutexLockError],
    Invalid => EINVAL,
    Permission => EPERM,
    Deadlock => EDEADLK,
    NotRecoverable => ENOTRECOVERABLE,
    Again => EAGAIN,
    TimedOut => ETIMEDOUT,
}

impl Display for MutexLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            // Descriptions based on https://man7.org/linux/man-pages/man3/pthread_mutex_lock.3p.html
            Self::Invalid => concat!(
                "Either the mutex was created with the protocol attribute having the value ",
                "PTHREAD_PRIO_PROTECT and the calling thread's priority is higher than ",
                "the mutex's current priority ceiling, or the value specified by mutex does ",
                "not refer to an initialised mutex object."
            ),
            Self::Permission => "The current thread does not own the mutex.",
            Self::Deadlock => "A deadlock condition was detected.",
            Self::NotRecoverable => "The state protected by the mutex is not recoverable.",
            Self::Again => concat!(
                "The mutex could not be acquired because the maximum number ",
                "of recursive locks for mutex has been exceeded."
            ),
            Self::TimedOut => "The mutex could not be acquired before the given deadline passed.",
            Self::Other(_) => "",
        };
        write_errcode(f, (*self).into(), message)
    }
}

impl Error for MutexLockError {}
