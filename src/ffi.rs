//! Declarations for the pthread entry points and constants that the `libc` crate does not bind on
//! every platform this crate supports.
//!
//! Every declaration was taken from the platform's own `pthread.h` rather than assumed, and the
//! `cfg`s here name the platforms that actually export the symbol. A declaration that is too
//! generous does not fail to compile: it fails to link, and only once something references it,
//! which is why `cargo check` cannot be trusted to police this file.
//!
//! Being careful matters most for the constants: Darwin numbers the process-shared values
//! differently from everybody else, so anything libc already provides is re-exported from libc
//! instead of being written out a second time.

// Not every declaration is reachable from every build configuration.
#![allow(dead_code, unused_imports)]

use libc::{c_int, pthread_mutexattr_t};

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
pub use pshared::*;

extern "C" {
    /// libc does not bind this for any platform this crate supports.
    pub fn pthread_mutexattr_gettype(attr: *const pthread_mutexattr_t, mtype: *mut c_int) -> c_int;

    /// Present on every platform this crate supports, though bionic only gained it at API
    /// level 28. libc binds it for Linux only.
    pub fn pthread_mutexattr_setprotocol(attr: *mut pthread_mutexattr_t, protocol: c_int) -> c_int;

    /// See [`pthread_mutexattr_setprotocol`].
    pub fn pthread_mutexattr_getprotocol(
        attr: *const pthread_mutexattr_t,
        protocol: *mut c_int,
    ) -> c_int;
}

// libc declares this with a `*mut` on some platforms and a `*const` on others, so this crate
// uses its own `*const` declaration to keep the attribute getters uniform.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
extern "C" {
    pub fn pthread_mutexattr_getrobust(
        attr: *const pthread_mutexattr_t,
        robustness: *mut c_int,
    ) -> c_int;
}

/// The mutex protocol values, which Linux, Darwin, FreeBSD, DragonFly, NetBSD and OpenBSD all
/// spell the same way. The unit tests below check them against libc wherever libc has an opinion.
pub const PTHREAD_PRIO_NONE: c_int = 0;
/// See [`PTHREAD_PRIO_NONE`].
pub const PTHREAD_PRIO_INHERIT: c_int = 1;
/// See [`PTHREAD_PRIO_NONE`]. bionic defines the other two but not this one, since Android has no
/// priority ceilings for the protocol to read.
pub const PTHREAD_PRIO_PROTECT: c_int = 2;

/// `PTHREAD_PROCESS_PRIVATE`, `PTHREAD_PROCESS_SHARED` and the three `*attr_setpshared` pairs, on
/// the platforms whose libraries implement process sharing. DragonFly, NetBSD and OpenBSD do not:
/// OpenBSD exports no pshared functions for mutexes or condvars at all, NetBSD hides its
/// declarations behind an off-by-default macro and returns `ENOSYS` for the shared value, and
/// DragonFly rejects it with `EINVAL`. Nothing here is compiled for those three, and neither is
/// anything that would reference it.
///
/// libc binds the six functions everywhere this module is compiled, Android included. The values
/// come from libc wherever libc has them, because Darwin numbers them differently from everybody
/// else: `SHARED` 1 and `PRIVATE` 2, against `PRIVATE` 0 and `SHARED` 1 elsewhere. `SHARED`
/// agrees across the two conventions, but a hardcoded `PRIVATE` would be rejected as invalid on
/// the other family. Android is the one platform where libc binds the functions but not the
/// values, so those are written out from bionic's `libc/include/pthread.h`.
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
mod pshared {
    pub use libc::{
        pthread_condattr_getpshared, pthread_condattr_setpshared, pthread_mutexattr_getpshared,
        pthread_mutexattr_setpshared, pthread_rwlockattr_getpshared, pthread_rwlockattr_setpshared,
    };

    #[cfg(not(target_os = "android"))]
    pub use libc::{PTHREAD_PROCESS_PRIVATE, PTHREAD_PROCESS_SHARED};

    /// Value checked against bionic's `libc/include/pthread.h`.
    #[cfg(target_os = "android")]
    pub const PTHREAD_PROCESS_PRIVATE: libc::c_int = 0;
    /// See [`PTHREAD_PROCESS_PRIVATE`].
    #[cfg(target_os = "android")]
    pub const PTHREAD_PROCESS_SHARED: libc::c_int = 1;
}

#[cfg(test)]
mod tests {
    /// The protocol values are written out by hand because libc does not bind them for every
    /// platform this crate supports. Where it does bind them, they had better agree.
    #[cfg(target_os = "linux")]
    #[test]
    fn protocol_constants_agree_with_libc() {
        assert_eq!(super::PTHREAD_PRIO_NONE, libc::PTHREAD_PRIO_NONE);
        assert_eq!(super::PTHREAD_PRIO_INHERIT, libc::PTHREAD_PRIO_INHERIT);
        assert_eq!(super::PTHREAD_PRIO_PROTECT, libc::PTHREAD_PRIO_PROTECT);
    }
}
