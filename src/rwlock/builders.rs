//! This module contains [`RwLockBuilder`] along with the various types used in its methods.

use std::mem::MaybeUninit;
use std::ptr;

use libc::{pthread_rwlock_t, pthread_rwlockattr_t};

use super::{BorrowedRwLock, OwnedRwLock, RawRwLockAlloc};
use crate::utils::AsRawUnderlying;

// Only the sharing attribute and the glibc-only preference go through `ffi`, and the platforms
// without process sharing have neither.
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use crate::ffi;

/// The main entrypoint for creating rwlocks. Internally, it initialises a `pthread_rwlockattr_t`
/// and decorates it with the attributes passed in from the `Self::with_*` methods, returning the
/// rwlock after a call to [`build_owned`](Self::build_owned) or
/// [`build_borrowed`](Self::build_borrowed).
///
/// If, instead, you already have a [`RawRwLockAlloc`] somewhere in memory and just need to
/// interpret it as a [`BorrowedRwLock`], use [`BorrowedRwLock::from_raw`].
pub struct RwLockBuilder {
    attr: pthread_rwlockattr_t,
}

impl RwLockBuilder {
    /// Creates a default `RwLockBuilder`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the process-shared attribute of the to-be constructed rwlock.
    /// See [`pthread_rwlockattr_setpshared`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_rwlockattr_setpshared.html)
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
    pub fn with_sharing(mut self, sharing: RwLockSharing) -> Self {
        unsafe {
            let r = ffi::pthread_rwlockattr_setpshared(self.as_mut_ptr(), sharing.into());
            debug_assert_eq!(r, 0);
        }
        self
    }

    /// Sets who wins when readers and writers contend for the to-be constructed rwlock.
    ///
    /// This is `pthread_rwlockattr_setkind_np`, a GNU extension rather than part of POSIX. bionic
    /// exports a variant of it with its own differently numbered constants, and FreeBSD declares
    /// it in `pthread.h` without any library defining it, so this crate offers it on glibc alone.
    #[cfg_attr(docsrs, doc(cfg(all(target_os = "linux", target_env = "gnu"))))]
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    pub fn with_preference(mut self, preference: RwLockPreference) -> Self {
        unsafe {
            let r = ffi::pthread_rwlockattr_setkind_np(self.as_mut_ptr(), preference.into());
            debug_assert_eq!(r, 0);
        }
        self
    }

    /// Constructs an [`OwnedRwLock`].
    pub fn build_owned(mut self) -> OwnedRwLock {
        let lock = OwnedRwLock::new_uninit();
        unsafe { initialize(lock.as_raw_underlying(), self.as_mut_ptr()) };
        lock
    }

    /// Constructs a [`BorrowedRwLock`] at a given location.
    ///
    /// - `raw`: A pointer to uninitialised data.
    ///
    /// - `memory`: A reference to a RAII object whose lifetime determines the validity of `raw`
    ///   (e.g. a struct that manages a memory map).
    ///
    /// # Safety
    /// The caller must guarantee that `raw` is correctly aligned, points to at least
    /// [`RawRwLockAlloc::SIZE`] writable bytes, and is not already in use by an initialised rwlock.
    pub unsafe fn build_borrowed<T>(
        mut self,
        raw: *mut RawRwLockAlloc,
        memory: &T,
    ) -> BorrowedRwLock<'_> {
        unsafe {
            let lock = BorrowedRwLock::from_raw(raw, memory);
            initialize(lock.as_raw_underlying(), self.as_mut_ptr());
            lock
        }
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut pthread_rwlockattr_t {
        ptr::addr_of_mut!(self.attr)
    }

    #[allow(dead_code)]
    pub(super) fn as_ptr(&self) -> *const pthread_rwlockattr_t {
        ptr::addr_of!(self.attr)
    }
}

unsafe fn initialize(lock: *mut pthread_rwlock_t, attr: *mut pthread_rwlockattr_t) {
    unsafe {
        let r = libc::pthread_rwlock_init(lock as *mut _, attr);
        debug_assert_eq!(r, 0);
    }
}

impl Default for RwLockBuilder {
    fn default() -> Self {
        let mut attr = MaybeUninit::<pthread_rwlockattr_t>::uninit();
        // # Safety
        // Calling `assume_init()` is safe because `pthread_rwlockattr_init` initialised the
        // attribute object in the line above it.
        unsafe {
            let r = libc::pthread_rwlockattr_init(attr.as_mut_ptr());
            debug_assert_eq!(r, 0);
            Self {
                attr: attr.assume_init(),
            }
        }
    }
}

impl Drop for RwLockBuilder {
    /// Releases the attribute object. The rwlocks built from it are unaffected:
    /// `pthread_rwlock_init` copies whatever it needs out of the attributes rather than holding
    /// onto them.
    fn drop(&mut self) {
        unsafe {
            let r = libc::pthread_rwlockattr_destroy(self.as_mut_ptr());
            debug_assert_eq!(r, 0);
        }
    }
}

/// Whether the rwlock may be used from more than one process.
/// See [pthread_rwlockattr_getpshared](https://man7.org/linux/man-pages/man3/pthread_rwlockattr_getpshared.3p.html)
/// for more information.
///
/// Not available on DragonFly, NetBSD or OpenBSD, none of which implements process sharing.
#[cfg_attr(
    docsrs,
    doc(cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))))
)]
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum RwLockSharing {
    /// The rwlock can not be shared across processes. Attempting to do so is undefined behaviour.
    #[default]
    Private,
    /// The rwlock can be shared across processes.
    Shared,
}

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
impl From<RwLockSharing> for i32 {
    fn from(value: RwLockSharing) -> Self {
        match value {
            RwLockSharing::Private => ffi::PTHREAD_PROCESS_PRIVATE,
            RwLockSharing::Shared => ffi::PTHREAD_PROCESS_SHARED,
        }
    }
}

/// Who wins when readers and writers contend for the lock.
///
/// This is a GNU extension rather than part of POSIX, which is why it is offered on glibc alone.
/// See [pthread_rwlockattr_setkind_np](https://man7.org/linux/man-pages/man3/pthread_rwlockattr_setkind_np.3.html)
/// for more information.
#[cfg_attr(docsrs, doc(cfg(all(target_os = "linux", target_env = "gnu"))))]
#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum RwLockPreference {
    /// A reader takes the lock even while a writer is waiting for it. This is the default, and a
    /// steady stream of readers can starve a writer indefinitely.
    #[default]
    Reader,
    /// Nominally writer-preferring, but glibc implements it the same way as
    /// [`Reader`](Self::Reader) in order to keep recursive read locking from deadlocking.
    Writer,
    /// Genuinely writer-preferring: a reader that asks for the lock while a writer is waiting
    /// blocks. That is what stops writer starvation, and it is also why taking a read lock
    /// recursively can deadlock.
    WriterNonRecursive,
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
impl From<RwLockPreference> for i32 {
    fn from(value: RwLockPreference) -> Self {
        match value {
            RwLockPreference::Reader => ffi::PTHREAD_RWLOCK_PREFER_READER_NP,
            RwLockPreference::Writer => ffi::PTHREAD_RWLOCK_PREFER_WRITER_NP,
            RwLockPreference::WriterNonRecursive => {
                ffi::PTHREAD_RWLOCK_PREFER_WRITER_NONRECURSIVE_NP
            }
        }
    }
}

// The only builder attributes to test are sharing and the glibc-only preference, so on the
// platforms without process sharing there is nothing left for this module to hold.
#[cfg(test)]
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
mod tests {
    use std::ptr;

    use super::{RwLockBuilder, RwLockSharing};
    use crate::ffi;

    /// Reads an attribute back out of a builder with one of the `pthread_rwlockattr_get*` getters.
    fn get<F>(builder: &RwLockBuilder, getter: F) -> i32
    where
        F: FnOnce(*const libc::pthread_rwlockattr_t, *mut i32) -> i32,
    {
        let mut out: i32 = -1;
        assert_eq!(getter(builder.as_ptr(), ptr::addr_of_mut!(out)), 0);
        out
    }

    #[test]
    fn sharing_is_properly_set_by_builder() {
        let getter = |attr, out| unsafe { ffi::pthread_rwlockattr_getpshared(attr, out) };

        let default = RwLockBuilder::new();
        assert_eq!(get(&default, getter), ffi::PTHREAD_PROCESS_PRIVATE);

        let shared = RwLockBuilder::new().with_sharing(RwLockSharing::Shared);
        assert_eq!(get(&shared, getter), ffi::PTHREAD_PROCESS_SHARED);
    }

    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    #[test]
    fn preference_is_properly_set_by_builder() {
        use super::RwLockPreference;

        let getter = |attr, out| unsafe { ffi::pthread_rwlockattr_getkind_np(attr, out) };

        for (preference, expected) in [
            (
                RwLockPreference::Reader,
                ffi::PTHREAD_RWLOCK_PREFER_READER_NP,
            ),
            (
                RwLockPreference::Writer,
                ffi::PTHREAD_RWLOCK_PREFER_WRITER_NP,
            ),
            (
                RwLockPreference::WriterNonRecursive,
                ffi::PTHREAD_RWLOCK_PREFER_WRITER_NONRECURSIVE_NP,
            ),
        ] {
            let builder = RwLockBuilder::new().with_preference(preference);
            assert_eq!(get(&builder, getter), expected);
        }
    }
}
