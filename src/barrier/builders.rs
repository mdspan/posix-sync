//! This module contains [`BarrierBuilder`] along with the various types used in its methods.

use std::mem::MaybeUninit;
use std::ptr;

use super::owned::HeapRawBarrier;
use super::{BarrierInitError, BorrowedBarrier, OwnedBarrier, RawBarrierAlloc};
use crate::ffi::{self, pthread_barrier_t, pthread_barrierattr_t};
use crate::utils::AsRawUnderlying;

/// The main entrypoint for creating barriers. Internally, it initialises a `pthread_barrierattr_t`
/// and decorates it with the attributes passed in from the `Self::with_*` methods, returning the
/// barrier after a call to [`build_owned`](Self::build_owned) or
/// [`build_borrowed`](Self::build_borrowed).
///
/// The number of threads the barrier waits for is an argument of those two methods, because POSIX
/// passes it to `pthread_barrier_init` directly and has no attribute for it.
///
/// If, instead, you already have a [`RawBarrierAlloc`] somewhere in memory and just need to
/// interpret it as a [`BorrowedBarrier`], use [`BorrowedBarrier::from_raw`].
pub struct BarrierBuilder {
    attr: pthread_barrierattr_t,
}

impl BarrierBuilder {
    /// Creates a default `BarrierBuilder`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the process-shared attribute of the to-be constructed barrier.
    /// See [`pthread_barrierattr_setpshared`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_barrierattr_setpshared.html)
    /// for more information.
    ///
    /// Not available on DragonFly, NetBSD or OpenBSD, none of which implements process sharing.
    #[cfg_attr(
        docsrs,
        doc(cfg(not(any(
            target_os = "dragonfly",
            target_os = "netbsd",
            target_os = "openbsd"
        ))))
    )]
    #[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
    pub fn with_sharing(mut self, sharing: BarrierSharing) -> Self {
        unsafe {
            let r = ffi::pthread_barrierattr_setpshared(self.as_mut_ptr(), sharing.into());
            debug_assert_eq!(r, 0);
        }
        self
    }

    /// Constructs an [`OwnedBarrier`] that releases its waiters in groups of `count`.
    ///
    /// # Errors
    /// See [`BarrierInitError`]. A `count` of zero is rejected everywhere.
    pub fn build_owned(self, count: u32) -> Result<OwnedBarrier, BarrierInitError> {
        let raw = HeapRawBarrier::new();
        unsafe { initialize(raw.as_raw_underlying(), self.as_ptr(), count)? };
        Ok(OwnedBarrier::from_initialised(raw))
    }

    /// Constructs a [`BorrowedBarrier`] at a given location.
    ///
    /// - `raw`: A pointer to uninitialised data.
    ///
    /// - `memory`: A reference to a RAII object whose lifetime determines the validity of `raw`
    ///   (e.g. a struct that manages a memory map).
    ///
    /// - `count`: How many threads, across every process using the barrier, have to wait on it
    ///   before any of them is released.
    ///
    /// # Safety
    /// The caller must guarantee that `raw` is correctly aligned, points to at least
    /// [`RawBarrierAlloc::SIZE`] writable bytes, and is not already in use by an initialised
    /// barrier.
    ///
    /// # Errors
    /// See [`BarrierInitError`]. A `count` of zero is rejected everywhere. On failure, the memory
    /// behind `raw` is left uninitialised.
    pub unsafe fn build_borrowed<T>(
        self,
        raw: *mut RawBarrierAlloc,
        memory: &T,
        count: u32,
    ) -> Result<BorrowedBarrier<'_>, BarrierInitError> {
        unsafe {
            let barrier = BorrowedBarrier::from_raw(raw, memory);
            initialize(barrier.as_raw_underlying(), self.as_ptr(), count)?;
            Ok(barrier)
        }
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut pthread_barrierattr_t {
        ptr::addr_of_mut!(self.attr)
    }

    pub(super) fn as_ptr(&self) -> *const pthread_barrierattr_t {
        ptr::addr_of!(self.attr)
    }
}

/// The other primitives in this crate only assert that their initialisation succeeded. A barrier's
/// can fail on a count the caller picked, so the error is handed back instead.
unsafe fn initialize(
    barrier: *mut pthread_barrier_t,
    attr: *const pthread_barrierattr_t,
    count: u32,
) -> Result<(), BarrierInitError> {
    unsafe {
        match ffi::pthread_barrier_init(barrier, attr, count) {
            0 => Ok(()),
            e => Err(BarrierInitError::from(e)),
        }
    }
}

impl Default for BarrierBuilder {
    fn default() -> Self {
        let mut attr = MaybeUninit::<pthread_barrierattr_t>::uninit();
        // # Safety
        // Calling `assume_init()` is safe because `pthread_barrierattr_init` initialised the
        // attribute object in the line above it.
        unsafe {
            let r = ffi::pthread_barrierattr_init(attr.as_mut_ptr());
            debug_assert_eq!(r, 0);
            Self {
                attr: attr.assume_init(),
            }
        }
    }
}

impl Drop for BarrierBuilder {
    /// Releases the attribute object. The barriers built from it are unaffected:
    /// `pthread_barrier_init` copies whatever it needs out of the attributes rather than holding
    /// onto them.
    fn drop(&mut self) {
        unsafe {
            let r = ffi::pthread_barrierattr_destroy(self.as_mut_ptr());
            debug_assert_eq!(r, 0);
        }
    }
}

/// Whether the barrier may be used from more than one process.
/// See [pthread_barrierattr_getpshared](https://man7.org/linux/man-pages/man3/pthread_barrierattr_getpshared.3p.html)
/// for more information.
///
/// Not available on DragonFly, NetBSD or OpenBSD, none of which implements process sharing.
#[cfg_attr(
    docsrs,
    doc(cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))))
)]
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum BarrierSharing {
    /// The barrier can not be shared across processes. Attempting to do so is undefined behaviour.
    #[default]
    Private,
    /// The barrier can be shared across processes.
    Shared,
}

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
impl From<BarrierSharing> for i32 {
    fn from(value: BarrierSharing) -> Self {
        match value {
            BarrierSharing::Private => ffi::PTHREAD_PROCESS_PRIVATE,
            BarrierSharing::Shared => ffi::PTHREAD_PROCESS_SHARED,
        }
    }
}

// The only builder attribute to test is sharing, so on the platforms without process sharing
// there is nothing left for this module to hold.
#[cfg(test)]
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
mod tests {
    use std::ptr;

    use super::{BarrierBuilder, BarrierSharing};
    use crate::ffi;

    /// Reads an attribute back out of a builder with one of the `pthread_barrierattr_get*`
    /// getters.
    fn get<F>(builder: &BarrierBuilder, getter: F) -> i32
    where
        F: FnOnce(*const ffi::pthread_barrierattr_t, *mut i32) -> i32,
    {
        let mut out: i32 = -1;
        assert_eq!(getter(builder.as_ptr(), ptr::addr_of_mut!(out)), 0);
        out
    }

    #[test]
    fn sharing_is_properly_set_by_builder() {
        let getter = |attr, out| unsafe { ffi::pthread_barrierattr_getpshared(attr, out) };

        let default = BarrierBuilder::new();
        assert_eq!(get(&default, getter), ffi::PTHREAD_PROCESS_PRIVATE);

        let shared = BarrierBuilder::new().with_sharing(BarrierSharing::Shared);
        assert_eq!(get(&shared, getter), ffi::PTHREAD_PROCESS_SHARED);
    }
}
