use std::error::Error;
use std::fmt::{Debug, Display};

use crate::utils::{impl_libc_error_conversions, write_errcode};

/// The error type returned when a barrier could not be initialised.
///
/// The variant descriptions and `Display` impls for this error type are based on info from the
/// POSIX man pages. The semantics, however, are platform-dependent. Consult the man pages of your
/// operating system for more relevant error descriptions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BarrierInitError {
    /// The system lacks the resources, other than memory, to initialise another barrier.
    Again,
    /// The count was out of range. POSIX requires rejecting a count of zero. glibc, musl, FreeBSD
    /// and DragonFly also reject counts above `i32::MAX`, and glibc rejects `i32::MAX` itself as
    /// well.
    Invalid,
    /// There was not enough memory to initialise the barrier.
    NoMemory,
    /// None of the above. Contains the libc error code.
    Other(i32),
}

impl_libc_error_conversions!(
    [BarrierInitError],
    Again => EAGAIN,
    Invalid => EINVAL,
    NoMemory => ENOMEM,
);

impl Display for BarrierInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            // Descriptions based on https://man7.org/linux/man-pages/man3/pthread_barrier_init.3p.html
            Self::Again => {
                "The system lacks the necessary resources to initialise another barrier."
            }
            Self::Invalid => "The value specified by count is equal to zero or too large.",
            Self::NoMemory => "Insufficient memory exists to initialise the barrier.",
            Self::Other(_) => "",
        };
        write_errcode(f, (*self).into(), message)
    }
}

impl Error for BarrierInitError {}

/// The error type returned from a wait on a barrier.
///
/// The variant descriptions and `Display` impls for this error type are based on info from the
/// POSIX man pages. The semantics, however, are platform-dependent. Consult the man pages of your
/// operating system for more relevant error descriptions.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BarrierWaitError {
    /// The value specified by the barrier does not refer to an initialised barrier object. POSIX
    /// defines no errors for `pthread_barrier_wait` and earlier editions only recommended
    /// reporting this one, so whether it is detected at all is up to the platform. This should
    /// not occur under normal circumstances.
    Invalid,
    /// None of the above. Contains the libc error code.
    Other(i32),
}

impl_libc_error_conversions!(
    [BarrierWaitError],
    Invalid => EINVAL,
);

impl Display for BarrierWaitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            // Descriptions based on https://man7.org/linux/man-pages/man3/pthread_barrier_wait.3p.html
            Self::Invalid => {
                "The value specified by barrier does not refer to an initialised barrier object."
            }
            Self::Other(_) => "",
        };
        write_errcode(f, (*self).into(), message)
    }
}

impl Error for BarrierWaitError {}
