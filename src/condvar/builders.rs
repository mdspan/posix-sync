//! This module contains [`CondvarBuilder`] along with the various types used in its methods.

use std::mem::MaybeUninit;
use std::ptr;

use libc::{pthread_cond_t, pthread_condattr_t};

use super::{BorrowedCondvar, CondvarClock, OwnedCondvar, RawCondvarAlloc};
// Only the sharing attribute goes through `ffi`; everything else here comes straight from libc.
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use crate::ffi;
use crate::utils::AsRawUnderlying;

/// The main entrypoint for creating condvars. Internally, it initialises a `pthread_condattr_t`
/// and decorates it with the attributes passed in from the `Self::with_*` methods, returning the
/// condvar after a call to [`build_owned`](Self::build_owned) or
/// [`build_borrowed`](Self::build_borrowed).
///
/// If, instead, you already have a [`RawCondvarAlloc`] somewhere in memory and just need to
/// interpret it as a [`BorrowedCondvar`], use [`BorrowedCondvar::from_raw`].
pub struct CondvarBuilder {
    attr: pthread_condattr_t,

    /// Kept alongside the attributes because the built condvar has to remember which clock its
    /// timed waits resolve deadlines against, and there is no portable way to ask it later.
    clock: CondvarClock,
}

impl CondvarBuilder {
    /// Creates a default `CondvarBuilder`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the process-shared attribute.
    /// See [`pthread_condattr_setpshared`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_condattr_setpshared.html)
    /// for more information.
    ///
    /// Not available on DragonFly, NetBSD or OpenBSD, which do not implement process sharing.
    #[cfg_attr(
        docsrs,
        doc(cfg(not(any(
            target_os = "dragonfly",
            target_os = "netbsd",
            target_os = "openbsd"
        ))))
    )]
    #[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
    pub fn with_sharing(mut self, sharing: CondvarSharing) -> Self {
        unsafe {
            let r = ffi::pthread_condattr_setpshared(self.as_mut_ptr(), sharing.into());
            debug_assert_eq!(r, 0);
        }
        self
    }

    /// Sets the clock the condvar resolves the deadlines of its timed waits against.
    /// See [`pthread_condattr_setclock`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_condattr_setclock.html)
    /// for more information.
    ///
    /// Not available on Apple platforms, which do not implement `pthread_condattr_setclock`. A
    /// condvar there always measures its timed waits against `CLOCK_REALTIME`.
    #[cfg_attr(docsrs, doc(cfg(not(target_vendor = "apple"))))]
    #[cfg(not(target_vendor = "apple"))]
    pub fn with_clock(mut self, clock: CondvarClock) -> Self {
        unsafe {
            let r = libc::pthread_condattr_setclock(self.as_mut_ptr(), clock.into());
            debug_assert_eq!(r, 0);
        }
        self.clock = clock;
        self
    }

    /// Constructs an [`OwnedCondvar`].
    pub fn build_owned(mut self) -> OwnedCondvar {
        let cv = OwnedCondvar::new_uninit(self.clock);
        unsafe { initialize(cv.as_raw_underlying(), self.as_mut_ptr()) };
        cv
    }

    /// Constructs a [`BorrowedCondvar`] at a given location.
    ///
    /// - `raw`: A pointer to uninitialised data.
    ///
    /// - `memory`: A reference to a RAII object whose lifetime determines the validity of `raw`
    ///   (e.g. a struct that manages a memory map).
    ///
    /// # Safety
    /// The caller must guarantee that `raw` is correctly aligned, points to at least
    /// [`RawCondvarAlloc::SIZE`] writable bytes, and is not already in use by an initialised
    /// condvar.
    pub unsafe fn build_borrowed<T>(
        mut self,
        raw: *mut RawCondvarAlloc,
        memory: &T,
    ) -> BorrowedCondvar<'_> {
        unsafe {
            let cv = BorrowedCondvar::from_raw(raw, memory, self.clock);
            initialize(cv.as_raw_underlying(), self.as_mut_ptr());
            cv
        }
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut pthread_condattr_t {
        ptr::addr_of_mut!(self.attr)
    }

    #[allow(dead_code)]
    pub(super) fn as_ptr(&self) -> *const pthread_condattr_t {
        ptr::addr_of!(self.attr)
    }
}

unsafe fn initialize(cv: *mut pthread_cond_t, attr: *mut pthread_condattr_t) {
    unsafe {
        let r = libc::pthread_cond_init(cv as *mut _, attr);
        debug_assert_eq!(r, 0);
    }
}

impl Default for CondvarBuilder {
    fn default() -> Self {
        let mut attr = MaybeUninit::<pthread_condattr_t>::uninit();
        // # Safety
        // Calling `assume_init()` is safe because `pthread_condattr_init` initialised the
        // attribute object in the line above it.
        unsafe {
            let r = libc::pthread_condattr_init(attr.as_mut_ptr());
            debug_assert_eq!(r, 0);
            Self {
                attr: attr.assume_init(),
                clock: CondvarClock::default(),
            }
        }
    }
}

impl Drop for CondvarBuilder {
    /// Releases the attribute object. The condvars built from it are unaffected: `pthread_cond_init`
    /// copies whatever it needs out of the attributes rather than holding onto them.
    fn drop(&mut self) {
        unsafe {
            let r = libc::pthread_condattr_destroy(self.as_mut_ptr());
            debug_assert_eq!(r, 0);
        }
    }
}

/// Whether the condvar may be used from more than one process.
/// See [pthread_condattr_getpshared](https://man7.org/linux/man-pages/man3/pthread_condattr_getpshared.3p.html)
/// for more information.
///
/// Not available on DragonFly, NetBSD or OpenBSD, none of which implements process sharing.
#[cfg_attr(
    docsrs,
    doc(cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))))
)]
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum CondvarSharing {
    /// The condvar can not be shared across processes. Attempting to do so is undefined behaviour.
    #[default]
    Private,
    /// The condvar can be shared across processes. The mutex it is waited on with has to be
    /// process-shared as well.
    Shared,
}

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
impl From<CondvarSharing> for i32 {
    fn from(value: CondvarSharing) -> Self {
        match value {
            CondvarSharing::Private => ffi::PTHREAD_PROCESS_PRIVATE,
            CondvarSharing::Shared => ffi::PTHREAD_PROCESS_SHARED,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;

    use super::CondvarBuilder;
    #[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
    use super::CondvarSharing;
    use crate::ffi;

    /// Reads an attribute back out of a builder with one of the `pthread_condattr_get*` getters.
    fn get<F>(builder: &CondvarBuilder, getter: F) -> i32
    where
        F: FnOnce(*const libc::pthread_condattr_t, *mut i32) -> i32,
    {
        let mut out: i32 = -1;
        assert_eq!(getter(builder.as_ptr(), ptr::addr_of_mut!(out)), 0);
        out
    }

    #[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
    #[test]
    fn sharing_is_properly_set_by_builder() {
        let getter = |attr, out| unsafe { ffi::pthread_condattr_getpshared(attr, out) };

        let default = CondvarBuilder::new();
        assert_eq!(get(&default, getter), ffi::PTHREAD_PROCESS_PRIVATE);

        let shared = CondvarBuilder::new().with_sharing(CondvarSharing::Shared);
        assert_eq!(get(&shared, getter), ffi::PTHREAD_PROCESS_SHARED);
    }

    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn clock_is_properly_set_by_builder() {
        use super::CondvarClock;

        let getter = |attr, out| unsafe { ffi::pthread_condattr_getclock(attr, out) };

        let default = CondvarBuilder::new();
        // NetBSD's attribute object stores no clock until the setter puts one there, and until
        // then its getter answers EINVAL rather than assuming CLOCK_REALTIME.
        #[cfg(not(target_os = "netbsd"))]
        assert_eq!(get(&default, getter), libc::CLOCK_REALTIME);
        assert_eq!(default.clock, CondvarClock::Realtime);

        let monotonic = CondvarBuilder::new().with_clock(CondvarClock::Monotonic);
        assert_eq!(get(&monotonic, getter), libc::CLOCK_MONOTONIC);

        // The builder also has to remember the clock, since the condvar cannot be asked later.
        assert_eq!(monotonic.clock, CondvarClock::Monotonic);
    }
}
