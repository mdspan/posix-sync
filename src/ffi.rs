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

// The priority ceiling pair belongs to the POSIX Thread Priority Protection option, which musl and
// bionic both leave unimplemented: neither exports the symbols, so a reference to one of these is
// a link error rather than a runtime failure. libc binds them nowhere, so they are written out
// here for the platforms that do have them.
#[cfg(not(any(target_env = "musl", target_os = "android")))]
extern "C" {
    pub fn pthread_mutexattr_setprioceiling(
        attr: *mut pthread_mutexattr_t,
        prioceiling: c_int,
    ) -> c_int;

    pub fn pthread_mutexattr_getprioceiling(
        attr: *const pthread_mutexattr_t,
        prioceiling: *mut c_int,
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

// libc leaves this unbound on NetBSD and OpenBSD, so it is declared here once for every platform
// that has it rather than mixing libc's bindings with our own.
#[cfg(not(target_vendor = "apple"))]
extern "C" {
    pub fn pthread_condattr_getclock(
        attr: *const libc::pthread_condattr_t,
        clock: *mut libc::clockid_t,
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

// POSIX specifies the timed rwlock functions, but Darwin never implemented them.
#[cfg(not(target_vendor = "apple"))]
extern "C" {
    pub fn pthread_rwlock_timedrdlock(
        rwlock: *mut libc::pthread_rwlock_t,
        abstime: *const libc::timespec,
    ) -> c_int;

    pub fn pthread_rwlock_timedwrlock(
        rwlock: *mut libc::pthread_rwlock_t,
        abstime: *const libc::timespec,
    ) -> c_int;
}

// The reader/writer preference pair is a GNU extension. bionic exports a variant of it too, with
// its own differently numbered constants, and FreeBSD's `pthread.h` declares the functions without
// any library defining them, so glibc is the only place this crate can offer it. libc binds the
// two functions for glibc, so they are re-exported rather than declared a second time; it binds
// none of the three constants, so those are written out from glibc's `pthread.h`.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub use libc::{pthread_rwlockattr_getkind_np, pthread_rwlockattr_setkind_np};

/// The reader/writer preference constants from glibc's `pthread.h`.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub const PTHREAD_RWLOCK_PREFER_READER_NP: c_int = 0;
/// See [`PTHREAD_RWLOCK_PREFER_READER_NP`].
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub const PTHREAD_RWLOCK_PREFER_WRITER_NP: c_int = 1;
/// See [`PTHREAD_RWLOCK_PREFER_READER_NP`].
#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub const PTHREAD_RWLOCK_PREFER_WRITER_NONRECURSIVE_NP: c_int = 2;

// POSIX puts barriers in an option of their own, which Darwin never implemented: its `unistd.h`
// defines `_POSIX_BARRIERS` as -1 and its `pthread.h` declares none of the functions. libc binds
// them for every other platform this crate supports except NetBSD and OpenBSD, so they are
// re-exported from libc wherever it has them and written out in `barrier` below for those two.
//
// libc only got DragonFly's two types right in 0.2.187. Earlier releases make
// `pthread_barrierattr_t` a `c_int`, which `pthread_barrierattr_init` then overruns with a
// pointer, and that is why `Cargo.toml` asks for 0.2.187 at least.
#[cfg(not(any(target_vendor = "apple", target_os = "netbsd", target_os = "openbsd")))]
pub use libc::{
    pthread_barrier_destroy, pthread_barrier_init, pthread_barrier_t, pthread_barrier_wait,
    pthread_barrierattr_destroy, pthread_barrierattr_init, pthread_barrierattr_t,
    PTHREAD_BARRIER_SERIAL_THREAD,
};

#[cfg(any(target_os = "netbsd", target_os = "openbsd"))]
pub use barrier::*;

/// The barrier types, functions and serial thread constant for NetBSD and OpenBSD, taken from
/// NetBSD's `lib/libpthread/pthread.h` and `pthread_types.h` and from OpenBSD's
/// `include/pthread.h`. Both libraries export all five functions declared here.
///
/// OpenBSD declares the attribute parameter of `pthread_barrier_init` without the `const` that
/// POSIX gives it. The pointer is passed the same way either way, so it is declared `*const` here
/// to match libc's signature on the other platforms.
#[cfg(any(target_os = "netbsd", target_os = "openbsd"))]
#[allow(non_camel_case_types)]
mod barrier {
    use libc::{c_int, c_uint, c_void};

    /// OpenBSD's barrier is a pointer to an object that `pthread_barrier_init` allocates.
    #[cfg(target_os = "openbsd")]
    pub type pthread_barrier_t = *mut c_void;
    /// See [`pthread_barrier_t`].
    #[cfg(target_os = "openbsd")]
    pub type pthread_barrierattr_t = *mut c_void;

    /// NetBSD's `struct __pthread_barrier_st`. Nothing in this crate reads the fields, which are
    /// only spelt out so that the size and alignment come out right. The two pointers in the
    /// middle are the `pthread_queue_t` of waiters.
    #[cfg(target_os = "netbsd")]
    #[repr(C)]
    pub struct pthread_barrier_t {
        ptb_magic: c_uint,
        ptb_lock: libc::pthread_spin_t,
        ptb_waiters_first: *mut c_void,
        ptb_waiters_last: *mut c_void,
        ptb_initcount: c_uint,
        ptb_curcount: c_uint,
        ptb_generation: c_uint,
        ptb_private: *mut c_void,
    }

    /// NetBSD's `struct __pthread_barrierattr_st`, spelt out for the same reason as
    /// [`pthread_barrier_t`].
    #[cfg(target_os = "netbsd")]
    #[repr(C)]
    pub struct pthread_barrierattr_t {
        ptba_magic: c_uint,
        ptba_private: *mut c_void,
    }

    /// NetBSD's value. Every other platform this crate supports uses -1.
    #[cfg(target_os = "netbsd")]
    pub const PTHREAD_BARRIER_SERIAL_THREAD: c_int = 1234567;
    /// OpenBSD's value, the -1 that every platform but NetBSD uses.
    #[cfg(target_os = "openbsd")]
    pub const PTHREAD_BARRIER_SERIAL_THREAD: c_int = -1;

    extern "C" {
        pub fn pthread_barrier_init(
            barrier: *mut pthread_barrier_t,
            attr: *const pthread_barrierattr_t,
            count: c_uint,
        ) -> c_int;

        pub fn pthread_barrier_destroy(barrier: *mut pthread_barrier_t) -> c_int;

        pub fn pthread_barrier_wait(barrier: *mut pthread_barrier_t) -> c_int;

        pub fn pthread_barrierattr_init(attr: *mut pthread_barrierattr_t) -> c_int;

        pub fn pthread_barrierattr_destroy(attr: *mut pthread_barrierattr_t) -> c_int;
    }
}

/// `PTHREAD_PROCESS_PRIVATE`, `PTHREAD_PROCESS_SHARED` and the four `*attr_setpshared` pairs, on
/// the platforms whose libraries implement process sharing. DragonFly, NetBSD and OpenBSD do not:
/// OpenBSD exports no pshared functions for mutexes or condvars at all, NetBSD hides its
/// declarations behind an off-by-default macro and returns `ENOSYS` for the shared value, and
/// DragonFly rejects it with `EINVAL`. Nothing here is compiled for those three, and neither is
/// anything that would reference it.
///
/// libc binds the eight functions everywhere this module is compiled, Android included, apart from
/// the barrier pair on Darwin, which has no barriers at all. The values come from libc wherever
/// libc has them, because Darwin numbers them differently from everybody else: `SHARED` 1 and
/// `PRIVATE` 2, against `PRIVATE` 0 and `SHARED` 1 elsewhere. `SHARED` agrees across the two
/// conventions, but a hardcoded `PRIVATE` would be rejected as invalid on the other family.
/// Android is the one platform where libc binds the functions but not the values, so those are
/// written out from bionic's `libc/include/pthread.h`.
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
mod pshared {
    pub use libc::{
        pthread_condattr_getpshared, pthread_condattr_setpshared, pthread_mutexattr_getpshared,
        pthread_mutexattr_setpshared, pthread_rwlockattr_getpshared, pthread_rwlockattr_setpshared,
    };

    #[cfg(not(target_vendor = "apple"))]
    pub use libc::{pthread_barrierattr_getpshared, pthread_barrierattr_setpshared};

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
