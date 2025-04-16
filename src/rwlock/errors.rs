use std::error::Error;
use std::fmt::{Debug, Display};

use crate::utils::{impl_libc_error_conversions, write_errcode};

/// The error type returned on failed rwlock locks.
///
/// The variant descriptions and `Display` impls for this error type are based on info from the
/// POSIX man pages. The semantics, however, are platform-dependent. Consult the man pages of your
/// operating system for more relevant error descriptions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RwLockError {
    /// The value specified by the rwlock does not refer to an initialised rwlock object. POSIX
    /// recommends rather than requires reporting this, so whether it is detected is up to the
    /// platform. It should not occur under normal circumstances.
    Invalid,
    /// The calling thread does not hold the rwlock. Locking never reports this, because unlocking
    /// is what the underlying `EPERM` describes and the guards here release the lock on drop
    /// rather than through a fallible method. It reaches callers only from a raw errno converted
    /// by hand.
    Permission,
    /// Locking would have resulted in a deadlock: the calling thread already holds the write lock,
    /// or it holds a read lock and asked for the write lock.
    Deadlock,
    /// The read lock could not be acquired because the maximum number of simultaneous readers has
    /// been exceeded.
    Again,
    /// The deadline of a timed lock passed before the lock could be acquired. The timed locking
    /// methods here report that outcome as `Ok(None)`, so this variant only shows up when
    /// a raw errno is converted by hand.
    TimedOut,
    /// None of the above. Contains the libc error code.
    Other(i32),
}

impl_libc_error_conversions! {
    [RwLockError],
    Invalid => EINVAL,
    Permission => EPERM,
    Deadlock => EDEADLK,
    Again => EAGAIN,
    TimedOut => ETIMEDOUT,
}

impl Display for RwLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            // Descriptions based on https://man7.org/linux/man-pages/man3/pthread_rwlock_rdlock.3p.html
            Self::Invalid => {
                "The value specified by rwlock does not refer to an initialised rwlock object."
            }
            Self::Permission => "The current thread does not hold a lock on the rwlock.",
            Self::Deadlock => "A deadlock condition was detected.",
            Self::Again => concat!(
                "The read lock could not be acquired because the maximum number of read locks ",
                "for rwlock has been exceeded."
            ),
            Self::TimedOut => "The lock could not be acquired before the given deadline passed.",
            Self::Other(_) => "",
        };
        write_errcode(f, (*self).into(), message)
    }
}

impl Error for RwLockError {}
