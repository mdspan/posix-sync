use std::error::Error;
use std::fmt::{Debug, Display};

use crate::utils::{impl_libc_error_conversions, write_errcode};

/// The error type returned from a notification on a condvar.
///
/// The variant descriptions and `Display` impls for this error type are based on info from the
/// POSIX man pages. The semantics, however, are platform-dependent. Consult the man pages of your
/// operating system for more relevant error descriptions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CondvarSignalError {
    /// The value specified by the condvar does not refer to an initialised condvar object. POSIX
    /// defines no errors for the notification functions and only recommends reporting this one,
    /// so whether it is detected at all is up to the platform. This should not occur under normal
    /// circumstances.
    Invalid,
    /// None of the above. Contains the libc error code.
    Other(i32),
}

impl_libc_error_conversions!(
    [CondvarSignalError],
    Invalid => EINVAL,
);

impl Display for CondvarSignalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            // Descriptions based on https://man7.org/linux/man-pages/man3/pthread_cond_signal.3p.html
            Self::Invalid => "The underlying pthread_cond_t object is not properly initialized.",
            Self::Other(_) => "",
        };
        write_errcode(f, (*self).into(), message)
    }
}

impl Error for CondvarSignalError {}

/// The error type returned from a wait or a timed wait on a condvar.
///
/// The variant descriptions and `Display` impls for this error type are based on info from the
/// POSIX man pages. The semantics, however, are platform-dependent. Consult the man pages of your
/// operating system for more relevant error descriptions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CondvarWaitError {
    /// The deadline of a timed wait passed before the condvar was notified. The timed waits here
    /// report that outcome as [`WaitOutcome::TimedOut`](super::WaitOutcome::TimedOut), so this
    /// variant only shows up when a raw errno is converted by hand.
    TimedOut,
    /// This error can occur for several reasons:
    ///
    /// - The deadline's nanosecond value was out of range. This is the only case POSIX requires
    ///   reporting.
    /// - The condvar or the mutex does not refer to an initialised object, or concurrent waits on
    ///   the same condvar were passed guards belonging to different mutexes. POSIX recommends
    ///   rather than requires reporting these, so whether they are detected is up to the platform.
    Invalid,
    /// The mutex is error-checking or robust and the calling thread does not own it. The waits in
    /// this crate take a guard, which is proof of ownership, so this variant only shows up when a
    /// raw errno is converted by hand.
    Permission,
    /// The mutex is robust and its previous owner terminated while holding the lock. The wait
    /// nevertheless returned with the mutex locked, so the guard is still good: repair the data
    /// it protects and call `mark_consistent` on it, or every later lock will fail with
    /// [`NotRecoverable`](crate::mutex::MutexLockError::NotRecoverable).
    OwnerDead,
    /// The mutex is robust, its previous owner died holding the lock, and nobody marked the data
    /// consistent afterwards.
    NotRecoverable,
    /// None of the above. Contains the libc error code.
    Other(i32),
}

impl_libc_error_conversions!(
    [CondvarWaitError],
    TimedOut => ETIMEDOUT,
    Invalid => EINVAL,
    Permission => EPERM,
    OwnerDead => EOWNERDEAD,
    NotRecoverable => ENOTRECOVERABLE,
);

impl Display for CondvarWaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            // Descriptions based on https://man7.org/linux/man-pages/man3/pthread_cond_timedwait.3p.html
            Self::TimedOut => "The deadline passed before the condition variable was notified.",
            Self::Invalid => concat!(
                "The abstime argument specified a nanosecond value less than zero or greater ",
                "than or equal to 1000 million, OR the value specified by cond or mutex does ",
                "not refer to an initialised object, OR different mutexes were supplied for ",
                "concurrent operations on the same condition variable."
            ),
            Self::Permission => concat!(
                "The mutex type is PTHREAD_MUTEX_ERRORCHECK or the mutex is a robust mutex, ",
                "and the current thread does not own the mutex."
            ),
            Self::OwnerDead => concat!(
                "The mutex is a robust mutex and the process containing the previous owning ",
                "thread terminated while holding the mutex lock. The mutex lock is acquired ",
                "by the calling thread and it is up to it to make the state consistent."
            ),
            Self::NotRecoverable => "The state protected by the mutex is not recoverable.",
            Self::Other(_) => "",
        };
        write_errcode(f, (*self).into(), message)
    }
}

impl Error for CondvarWaitError {}
