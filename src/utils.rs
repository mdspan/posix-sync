//! Odds and ends shared by the primitives in this crate.

use std::fmt;
use std::mem::{size_of_val, MaybeUninit};
use std::time::Duration;

use libc::{clockid_t, timespec};

#[allow(missing_docs)]
pub trait Sealed {}

/// Implements conversions to and from libc error codes for an error enum. It expects that the enum
/// has an `Other(i32)` variant.
macro_rules! impl_libc_error_conversions {
    ([$error_type:ty], $($variant:ident => $libc_const:ident),* $(,)?) => {
        impl ::core::convert::From<i32> for $error_type {
            fn from(value: i32) -> Self {
                match value {
                    $(::libc::$libc_const => Self::$variant,)*
                    _ => Self::Other(value),
                }
            }
        }
        impl ::core::convert::From<$error_type> for i32 {
            fn from(value: $error_type) -> Self {
                use $error_type::{Other};
                match value {
                    $(<$error_type>::$variant => ::libc::$libc_const,)*
                    Other(val) => val,
                }
            }
        }
    };
}

pub(crate) use impl_libc_error_conversions;

/// Renders one of this crate's error types as `Errcode N. Message`. The `Other` variants carry no
/// message of their own, so the separating space is only written when there is something after it.
pub(crate) fn write_errcode(f: &mut fmt::Formatter<'_>, code: i32, message: &str) -> fmt::Result {
    write!(f, "Errcode {code}.")?;
    if !message.is_empty() {
        write!(f, " {message}")?;
    }
    Ok(())
}

/// Implemented by types which manage some underlying FFI object via interior mutability through a
/// `*mut`.
pub trait AsRawUnderlying: Sealed {
    type Underlying;

    fn as_raw_underlying(&self) -> *mut Self::Underlying;
}

/// Returns the point in time, as measured by `clock`, that lies `timeout` in the future.
///
/// The `pthread_*_timed*` family takes absolute deadlines, so every relative timeout in this crate
/// has to be resolved against the clock the primitive was created with. The arithmetic saturates,
/// which turns an absurdly long timeout into a deadline that will not realistically be reached
/// rather than one that has already passed.
// `tv_sec` and `tv_nsec` are `time_t` and `c_long`, which are 64-bit on some targets and 32-bit on
// others. The casts below are redundant only on the former.
#[allow(clippy::unnecessary_cast)]
pub(crate) fn deadline_from_now(clock: clockid_t, timeout: Duration) -> timespec {
    const NANOS_PER_SEC: i64 = 1_000_000_000;

    let mut deadline = MaybeUninit::<timespec>::uninit();
    // The only documented failure is an unsupported clock, and every clock id reaching this
    // function names one the platform is known to have.
    let r = unsafe { libc::clock_gettime(clock, deadline.as_mut_ptr()) };
    debug_assert_eq!(r, 0);
    let mut deadline = unsafe { deadline.assume_init() };

    let mut secs = (deadline.tv_sec as i64)
        .saturating_add(i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX));
    let mut nanos = deadline.tv_nsec as i64 + i64::from(timeout.subsec_nanos());
    if nanos >= NANOS_PER_SEC {
        nanos -= NANOS_PER_SEC;
        secs = secs.saturating_add(1);
    }

    // `tv_sec` is a `time_t`, which is 32 bits wide on some targets and 64 on others. Its width
    // is read off the field itself rather than through `libc::time_t`, which musl deprecates.
    let tv_sec_max = if size_of_val(&deadline.tv_sec) >= 8 {
        i64::MAX
    } else {
        i64::from(i32::MAX)
    };

    deadline.tv_sec = secs.min(tv_sec_max) as _;
    deadline.tv_nsec = nanos as _;
    deadline
}
