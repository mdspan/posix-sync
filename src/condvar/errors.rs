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
    /// The value specified by the condvar does not refer to an initialized condvar object. POSIX
    /// only recommends reporting this error, so not every platform detects it. This should not
    /// occur under normal circumstances.
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
    /// The deadline of a timed wait passed before the condvar was notified. The timed waits in
    /// this crate return [`WaitOutcome::TimedOut`](super::WaitOutcome::TimedOut) instead, so this
    /// variant only appears when converting a raw error code.
    TimedOut,
    /// This error can occur for several reasons:
    ///
    /// - The deadline's nanosecond value was out of range. POSIX requires reporting this case.
    /// - The condvar or the mutex does not refer to an initialized object, or concurrent waits on
    ///   the same condvar used guards belonging to different mutexes. POSIX only recommends
    ///   reporting these cases, so not every platform detects them.
    Invalid,
    /// The mutex is error-checking or robust and the calling thread does not own it. The waits in
    /// this crate take a guard, so this variant only appears when converting a raw error code.
    Permission,
    /// The mutex is robust and its previous owner terminated while holding the lock. The wait
    /// returned with the mutex locked, so the guard is still valid. If the protected data is
    /// intact, or can be repaired here, call `mark_consistent` on the guard. Otherwise, every
    /// later lock fails with [`NotRecoverable`](crate::mutex::MutexLockError::NotRecoverable).
    OwnerDead,
    /// The mutex is robust, its previous owner terminated while holding the lock, and the data
    /// was not marked consistent afterwards.
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
                "not refer to an initialized object, OR different mutexes were supplied for ",
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
