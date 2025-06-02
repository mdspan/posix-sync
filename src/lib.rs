//! Rust bindings to the POSIX synchronisation primitives: mutexes, condition variables and
//! reader/writer locks, with their attributes and RAII guards.
//!
//! A lock can be placed at an address you choose.
//! This is useful for embedding synchronisation primitives into mapped memory.
//! Some use cases, which aren't handled by `std::sync::Mutex`, include:
//!
//! - **Shared memory.** Two processes mapping the same region need the lock to live inside it.
//! - **A holder that dies.** A robust mutex hands the next locker `EOWNERDEAD`, so it can repair
//!   the state.
//! - **Real-time scheduling.** Priority inheritance and priority ceilings bound how long a
//!   low-priority holder can stall a high-priority waiter.
//! - **Portability.** Process-shared locks are specified by POSIX rather than by any one kernel,
//!   so one implementation covers every platform that provides them. The alternative is a futex or
//!   another OS-specific primitive, rewritten per target.
//! - **Relocking.** Error-checking and recursive types make a second lock on the same thread
//!   report a deadlock or succeed.
//!
//! Each primitive comes in two flavours:
//!
//! - An *owned* variant, which allocates its own pthread object, destroys it on drop as long as
//!   nothing still holds it, and can be used entirely from safe code.
//! - A *borrowed* variant, which points at memory you supply, is `Copy`, destroys nothing
//!   implicitly, and has `unsafe` methods.
//!
//! | Module | Owned | Borrowed | Attributes |
//! |--------|-------|----------|------------|
//! | [`mutex`] | [`OwnedMutex`](mutex::OwnedMutex) | [`BorrowedMutex`](mutex::BorrowedMutex) | sharing, robustness, type, protocol, priority ceiling |
//! | [`condvar`] | [`OwnedCondvar`](condvar::OwnedCondvar) | [`BorrowedCondvar`](condvar::BorrowedCondvar) | sharing, clock |
//! | [`rwlock`] | [`OwnedRwLock`](rwlock::OwnedRwLock) | [`BorrowedRwLock`](rwlock::BorrowedRwLock) | sharing, reader/writer preference |
//!
//! All three can be process-shared, as long as the platform supports it (see the table below).
//! There is no
//! [poisoning](https://doc.rust-lang.org/std/sync/poison/struct.Mutex.html#poisoning), which
//! makes them `!UnwindSafe` and `!RefUnwindSafe`.
//!
//! # Platform support
//!
//! | | Linux (glibc) | Linux (musl) | FreeBSD | DragonFly | NetBSD | OpenBSD | macOS/iOS | Android |
//! |---|---|---|---|---|---|---|---|---|
//! | Process sharing | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | ✅ | ✅ |
//! | Robust mutexes | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
//! | Timed mutex locking | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ |
//! | Timed rwlock locking | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ |
//! | Condvar clock selection | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ |
//! | Priority inheritance | ✅ | ✅ | ✅ | ✅ | ❌ | ✅ | ✅ | ✅ |
//! | Priority ceilings | ✅ | ❌ | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
//! | Reader/writer preference | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ | ❌ |
//!
//! - **Process sharing.** `with_sharing` and the `*Sharing` enums on all three builders.
//!   DragonFly, NetBSD and OpenBSD never implemented the option: OpenBSD's libraries export no
//!   pshared functions for mutexes or condvars at all, NetBSD's return `ENOSYS` for the shared
//!   value, and DragonFly's reject it with `EINVAL`. Where the row is ❌, `with_sharing` and the
//!   `*Sharing` enums do not exist, so the locks there can only synchronise threads within a
//!   single process, never two processes over shared memory.
//! - **Robust mutexes.** macOS/iOS, NetBSD, OpenBSD and Android do not implement robust mutexes
//!   at all. DragonFly declares the robust functions but does not define them.
//! - **Timed mutex locking.** macOS/iOS has no `pthread_mutex_timedlock`.
//! - **Timed rwlock locking.** macOS/iOS never implemented the timed rwlock functions.
//! - **Condvar clock selection.** macOS/iOS has no `pthread_condattr_setclock`, so a condvar
//!   there always measures its timed waits against `CLOCK_REALTIME`.
//! - **Priority inheritance.** NetBSD rejects `PTHREAD_PRIO_INHERIT` with `ENOTSUP` while
//!   accepting the other two protocols. On Android the protocol functions only exist from API
//!   level 28, so selecting any protocol there needs a target at least that new.
//! - **Priority ceilings.** The musl and Android C libraries leave the POSIX Thread Priority
//!   Protection option unimplemented: neither exports `pthread_mutexattr_setprioceiling`, and
//!   both reject `PTHREAD_PRIO_PROTECT`.
//! - **Reader/writer preference.** Choosing who wins when readers and writers contend is a GNU
//!   extension (`pthread_rwlockattr_setkind_np`) rather than part of POSIX. Android's C library
//!   exports a variant with its own differently numbered constants, and FreeBSD declares the
//!   functions without defining them, so this crate offers the preference on glibc alone.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
#![warn(rustdoc::all)]

pub(crate) mod ffi;
pub(crate) mod utils;

pub mod condvar;
pub mod mutex;
pub mod rwlock;

/// Compiles the code blocks in the README, so that the front page cannot drift from the API
/// it advertises. `cfg(doctest)` holds only while rustdoc is collecting doctests, so this is
/// invisible to every other build.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
