//! Odds and ends shared by the primitives in this crate.

use std::fmt;

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
