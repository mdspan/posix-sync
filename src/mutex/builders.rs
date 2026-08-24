//! This module contains [`MutexBuilder`] along with the various types used in its methods.

use std::marker::PhantomData;
use std::mem::MaybeUninit;
#[cfg(not(any(target_env = "musl", target_os = "android")))]
use std::ops::RangeInclusive;
use std::ptr;

use libc::{pthread_mutex_t, pthread_mutexattr_t};

use super::robustness_markers::RobustnessMarker;
use super::{BorrowedMutex, OwnedMutex, RawMutexAlloc};
use crate::ffi;
use crate::utils::AsRawUnderlying;

/// The main entrypoint for creating mutexes. Internally, it initialises a `pthread_mutexattr_t`
/// and decorates it with the attributes passed in from the `Self::with_*` methods, returning the
/// mutex after a call to [`build_owned`](Self::build_owned) or [`build_borrowed`](Self::build_borrowed).
///
/// If, instead, you already have a [`RawMutexAlloc`] somewhere in memory and just need to
/// interpret it as a [`BorrowedMutex`], use [`BorrowedMutex::from_raw`].
pub struct MutexBuilder<R: RobustnessMarker> {
    attr: pthread_mutexattr_t,
    _phantom: PhantomData<R>,
}

impl<R: RobustnessMarker> MutexBuilder<R> {
    /// Creates a default `MutexBuilder`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the process-shared attribute of the to-be constructed mutex.
    /// See [`pthread_mutexattr_setpshared`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_mutexattr_setpshared.html)
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
    pub fn with_sharing(mut self, sharing: MutexSharing) -> Self {
        unsafe {
            let r = ffi::pthread_mutexattr_setpshared(self.as_mut_ptr(), sharing.into());
            debug_assert_eq!(r, 0);
        }
        self
    }

    /// Sets the protocol attribute of the to-be constructed mutex.
    /// See [`pthread_mutexattr_setprotocol`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_mutexattr_setprotocol.html)
    /// for more information.
    ///
    /// On Android the protocol functions only exist from API level 28, so calling this on an
    /// older target fails to link.
    pub fn with_protocol(mut self, protocol: MutexProtocol) -> Self {
        unsafe {
            let r = ffi::pthread_mutexattr_setprotocol(self.as_mut_ptr(), protocol.into());
            debug_assert_eq!(r, 0);
        }
        self
    }

    /// Sets the mutex type attribute of the to-be constructed mutex.
    /// See [`pthread_mutexattr_settype`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_mutexattr_settype.html)
    /// for more information.
    pub fn with_type(mut self, mtype: MutexType) -> Self {
        unsafe {
            let r = libc::pthread_mutexattr_settype(self.as_mut_ptr(), mtype.into());
            debug_assert_eq!(r, 0);
        }
        self
    }

    /// Sets the priority ceiling of the to-be constructed mutex, which only has an effect on a
    /// mutex built with [`MutexProtocol::Protect`]. Values outside of
    /// [`priority_ceiling_range`] are clamped into it.
    /// See [`pthread_mutexattr_setprioceiling`](https://pubs.opengroup.org/onlinepubs/9799919799/functions/pthread_mutexattr_setprioceiling.html)
    /// for more information.
    ///
    /// On FreeBSD and DragonFly the attribute object refuses to hold a ceiling while its
    /// protocol is anything other than [`MutexProtocol::Protect`], so apply
    /// [`with_protocol`](Self::with_protocol) first there.
    ///
    /// Not available on musl or Android, neither of which implements the POSIX Thread Priority
    /// Protection option.
    #[cfg_attr(docsrs, doc(cfg(not(any(target_env = "musl", target_os = "android")))))]
    #[cfg(not(any(target_env = "musl", target_os = "android")))]
    pub fn with_priority_ceiling(mut self, ceiling: i32) -> Self {
        let range = priority_ceiling_range();
        let ceiling = ceiling.clamp(*range.start(), *range.end());
        unsafe {
            let r = ffi::pthread_mutexattr_setprioceiling(self.as_mut_ptr(), ceiling);
            debug_assert_eq!(r, 0);
        }
        self
    }

    /// Constructs an [`OwnedMutex`].
    pub fn build_owned(mut self) -> OwnedMutex<R> {
        let mtx = OwnedMutex::new_uninit();
        unsafe { initialize(mtx.as_raw_underlying(), self.as_mut_ptr()) };
        mtx
    }

    /// Constructs a [`BorrowedMutex`] at a given location.
    ///
    /// - `raw`: A pointer to uninitialised data.
    ///
    /// - `memory`: A reference to a RAII object whose lifetime determines the validity of
    ///   `raw` (e.g. a struct that manages a memory map).
    ///
    /// # Safety
    /// The caller must guarantee that `raw` is correctly aligned, points to at least
    /// [`RawMutexAlloc::SIZE`] writable bytes, and is not already in use by an initialised mutex.
    pub unsafe fn build_borrowed<T>(
        mut self,
        raw: *mut RawMutexAlloc,
        memory: &T,
    ) -> BorrowedMutex<'_, R> {
        unsafe {
            let mtx = BorrowedMutex::from_raw(raw, memory);
            initialize(mtx.as_raw_underlying(), self.as_mut_ptr());
            mtx
        }
    }

    pub(super) fn as_mut_ptr(&mut self) -> *mut pthread_mutexattr_t {
        ptr::addr_of_mut!(self.attr)
    }

    #[allow(dead_code)]
    pub(super) fn as_ptr(&self) -> *const pthread_mutexattr_t {
        ptr::addr_of!(self.attr)
    }
}

/// The values [`MutexBuilder::with_priority_ceiling`] accepts, which is the priority range of the
/// `SCHED_FIFO` policy on this platform.
///
/// Not available on musl or Android, neither of which implements the POSIX Thread Priority
/// Protection option.
#[cfg_attr(docsrs, doc(cfg(not(any(target_env = "musl", target_os = "android")))))]
#[cfg(not(any(target_env = "musl", target_os = "android")))]
pub fn priority_ceiling_range() -> RangeInclusive<i32> {
    unsafe {
        libc::sched_get_priority_min(libc::SCHED_FIFO)
            ..=libc::sched_get_priority_max(libc::SCHED_FIFO)
    }
}

unsafe fn initialize(mtx: *mut pthread_mutex_t, attr: *mut pthread_mutexattr_t) {
    unsafe {
        let r = libc::pthread_mutex_init(mtx as *mut _, attr);
        debug_assert_eq!(r, 0);
    }
}

impl<R: RobustnessMarker> Default for MutexBuilder<R> {
    fn default() -> Self {
        let mut attr = MaybeUninit::<pthread_mutexattr_t>::uninit();
        // # Safety
        // Calling `assume_init()` is safe because `pthread_mutexattr_init` initialised the
        // attribute object in the line above it.
        unsafe {
            let r = libc::pthread_mutexattr_init(attr.as_mut_ptr());
            debug_assert_eq!(r, 0);
            // Platforms without robust mutexes have no robustness attribute to set, and on
            // those `Standard` is the only marker that exists.
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            {
                let r = libc::pthread_mutexattr_setrobust(
                    attr.as_mut_ptr(),
                    <R as RobustnessMarker>::VALUE,
                );
                debug_assert_eq!(r, 0);
            }
            Self {
                attr: attr.assume_init(),
                _phantom: PhantomData,
            }
        }
    }
}

impl<R: RobustnessMarker> Drop for MutexBuilder<R> {
    /// Releases the attribute object. The mutexes built from it are unaffected: `pthread_mutex_init`
    /// copies whatever it needs out of the attributes rather than holding onto them.
    fn drop(&mut self) {
        unsafe {
            let r = libc::pthread_mutexattr_destroy(self.as_mut_ptr());
            debug_assert_eq!(r, 0);
        }
    }
}

/// The type of the mutex.
/// See [pthread_mutex_lock](https://man7.org/linux/man-pages/man3/pthread_mutex_lock.3p.html)
/// for more information.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum MutexType {
    /// If a thread already holds a lock and attempts to lock the mutex again,
    /// the POSIX standard mandates that a deadlock occur. However, some platforms will return a
    /// [`MutexLockError::Deadlock`](super::errors::MutexLockError::Deadlock) despite the mutex not
    /// being error-checked.
    #[default]
    Normal,
    /// If a thread already holds a lock and attempts to lock the mutex again,
    /// [`MutexLockError::Deadlock`](super::errors::MutexLockError::Deadlock) is returned.
    ErrorCheck,
    /// If a thread already holds a lock and attempts to lock the mutex again,
    /// the call will succeed as long as the recursive lock limit is not exceeded.
    Recursive,
}

impl From<MutexType> for i32 {
    fn from(value: MutexType) -> Self {
        match value {
            MutexType::Normal => libc::PTHREAD_MUTEX_NORMAL,
            MutexType::ErrorCheck => libc::PTHREAD_MUTEX_ERRORCHECK,
            MutexType::Recursive => libc::PTHREAD_MUTEX_RECURSIVE,
        }
    }
}

/// Whether the mutex may be used from more than one process.
/// See [pthread_mutexattr_getpshared](https://man7.org/linux/man-pages/man3/pthread_mutexattr_getpshared.3.html)
/// for more information.
///
/// Not available on DragonFly, NetBSD or OpenBSD, none of which implements process sharing.
#[cfg_attr(
    docsrs,
    doc(cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd"))))
)]
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum MutexSharing {
    /// The mutex can not be shared across processes. Attempting to do so is undefined
    /// behaviour.
    #[default]
    Private,
    /// The mutex can be shared across processes.
    Shared,
}

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
impl From<MutexSharing> for i32 {
    fn from(value: MutexSharing) -> Self {
        match value {
            MutexSharing::Private => ffi::PTHREAD_PROCESS_PRIVATE,
            MutexSharing::Shared => ffi::PTHREAD_PROCESS_SHARED,
        }
    }
}

/// The protocol to be used with the mutex.
/// See [pthread_mutexattr_getprotocol](https://man7.org/linux/man-pages/man3/pthread_mutexattr_getprotocol.3p.html)
/// for more information.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum MutexProtocol {
    /// The lock-owning thread's priority is unaffected by the mutex.
    #[default]
    None,
    /// The lock-owning thread's priority is the maximum of its own priority
    /// and the priority of all threads waiting on the mutex.
    ///
    /// Not available on NetBSD, whose `pthread_mutexattr_setprotocol` rejects
    /// `PTHREAD_PRIO_INHERIT` with `ENOTSUP` while accepting the other two protocols.
    #[cfg_attr(docsrs, doc(cfg(not(target_os = "netbsd"))))]
    #[cfg(not(target_os = "netbsd"))]
    Inherit,
    /// The lock-owning thread's priority is the maximum of its own priority and the priority
    /// ceilings of every mutex it holds that was built with this protocol, whether or not anyone
    /// is blocked on them.
    ///
    /// Set the ceilings with [`MutexBuilder::with_priority_ceiling`]. Not available on musl or
    /// Android, neither of which implements the POSIX Thread Priority Protection option: there are
    /// no ceilings there for this protocol to read, and both reject it outright.
    #[cfg_attr(docsrs, doc(cfg(not(any(target_env = "musl", target_os = "android")))))]
    #[cfg(not(any(target_env = "musl", target_os = "android")))]
    Protect,
}

impl From<MutexProtocol> for i32 {
    fn from(value: MutexProtocol) -> Self {
        match value {
            #[cfg(not(target_os = "netbsd"))]
            MutexProtocol::Inherit => ffi::PTHREAD_PRIO_INHERIT,
            MutexProtocol::None => ffi::PTHREAD_PRIO_NONE,
            #[cfg(not(any(target_env = "musl", target_os = "android")))]
            MutexProtocol::Protect => ffi::PTHREAD_PRIO_PROTECT,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ptr;

    #[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
    use super::MutexSharing;
    use super::{MutexBuilder, MutexProtocol, MutexType};
    use crate::ffi;
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    use crate::mutex::robustness_markers::Robust;
    use crate::mutex::robustness_markers::{RobustnessMarker, Standard};

    /// Reads an attribute back out of a builder with one of the `pthread_mutexattr_get*` getters.
    fn get<R, F>(builder: &MutexBuilder<R>, getter: F) -> i32
    where
        R: RobustnessMarker,
        F: FnOnce(*const libc::pthread_mutexattr_t, *mut i32) -> i32,
    {
        let mut out: i32 = -1;
        assert_eq!(getter(builder.as_ptr(), ptr::addr_of_mut!(out)), 0);
        out
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn robustness_is_properly_set_by_builder() {
        let standard = MutexBuilder::<Standard>::new();
        let robust = MutexBuilder::<Robust>::new();

        let getter = |attr, out| unsafe { ffi::pthread_mutexattr_getrobust(attr, out) };
        assert_eq!(get(&standard, getter), libc::PTHREAD_MUTEX_STALLED);
        assert_eq!(get(&robust, getter), libc::PTHREAD_MUTEX_ROBUST);
    }

    #[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
    #[test]
    fn sharing_is_properly_set_by_builder() {
        let getter = |attr, out| unsafe { ffi::pthread_mutexattr_getpshared(attr, out) };

        let default = MutexBuilder::<Standard>::new();
        assert_eq!(get(&default, getter), ffi::PTHREAD_PROCESS_PRIVATE);

        let shared = MutexBuilder::<Standard>::new().with_sharing(MutexSharing::Shared);
        assert_eq!(get(&shared, getter), ffi::PTHREAD_PROCESS_SHARED);
    }

    #[test]
    fn type_is_properly_set_by_builder() {
        let getter = |attr, out| unsafe { ffi::pthread_mutexattr_gettype(attr, out) };

        for (mtype, expected) in [
            (MutexType::Normal, libc::PTHREAD_MUTEX_NORMAL),
            (MutexType::ErrorCheck, libc::PTHREAD_MUTEX_ERRORCHECK),
            (MutexType::Recursive, libc::PTHREAD_MUTEX_RECURSIVE),
        ] {
            let builder = MutexBuilder::<Standard>::new().with_type(mtype);
            assert_eq!(get(&builder, getter), expected);
        }
    }

    #[test]
    fn protocol_is_properly_set_by_builder() {
        let getter = |attr, out| unsafe { ffi::pthread_mutexattr_getprotocol(attr, out) };

        for (protocol, expected) in [
            (MutexProtocol::None, ffi::PTHREAD_PRIO_NONE),
            #[cfg(not(target_os = "netbsd"))]
            (MutexProtocol::Inherit, ffi::PTHREAD_PRIO_INHERIT),
            #[cfg(not(any(target_env = "musl", target_os = "android")))]
            (MutexProtocol::Protect, ffi::PTHREAD_PRIO_PROTECT),
        ] {
            let builder = MutexBuilder::<Standard>::new().with_protocol(protocol);
            assert_eq!(get(&builder, getter), expected);
        }
    }

    #[cfg(not(any(target_env = "musl", target_os = "android")))]
    #[test]
    fn priority_ceiling_is_properly_set_by_builder() {
        use super::priority_ceiling_range;

        let getter = |attr, out| unsafe { ffi::pthread_mutexattr_getprioceiling(attr, out) };
        let range = priority_ceiling_range();

        // FreeBSD and DragonFly refuse to get or set a ceiling while the protocol is not
        // Protect, so the protocol goes in first.
        let protect = || MutexBuilder::<Standard>::new().with_protocol(MutexProtocol::Protect);

        let builder = protect().with_priority_ceiling(*range.start());
        assert_eq!(get(&builder, getter), *range.start());

        // Out of range ceilings are clamped rather than rejected, so the attribute object is never
        // left holding a value the platform would refuse.
        let builder = protect().with_priority_ceiling(i32::MAX);
        assert_eq!(get(&builder, getter), *range.end());
    }
}
