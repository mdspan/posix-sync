# posix-sync

[![CI](https://github.com/mdspan/posix-sync/actions/workflows/ci.yml/badge.svg)](https://github.com/mdspan/posix-sync/actions/workflows/ci.yml)

Idiomatic Rust interface for POSIX synchronisation primitives.

Every primitive comes in two flavours:

- An **owned** variant, which allocates the underlying pthread object, destroys it on
  drop as long as nothing still holds it, and can be used entirely from safe code
- A **borrowed** variant, which points at memory you supply (e.g. a shared mapping),
  is `Copy`, never destroys anything, and whose methods are `unsafe` because
  another process may invalidate the underlying object

```rust
use posix_sync::mutex::{MutexBuilder, MutexType, robustness_markers::Standard};

let mtx = MutexBuilder::<Standard>::new()
    .with_type(MutexType::ErrorCheck)
    .build_owned();

let guard = mtx.lock().unwrap();
drop(guard);
```

Robustness is a type parameter rather than an attribute, so the guard type follows from
it and the two kinds of lock cannot be mixed up. A robust mutex hands the next locker a
guard that has to be matched on:

```rust,ignore
use posix_sync::mutex::{
    MutexBuilder, guards::RobustGuardContainer, robustness_markers::Robust,
};

let mtx = MutexBuilder::<Robust>::new().build_owned();

match mtx.lock().unwrap() {
    RobustGuardContainer::Standard(_guard) => {}
    RobustGuardContainer::Indeterminate(guard) => {
        // The previous owner died holding the lock. Repair the data, then:
        let _guard = guard.make_consistent().unwrap();
    }
}
```

That block is marked `ignore` because `Robust` only exists on the platforms in the table
below. The [`mutex` module docs](https://docs.rs/posix-sync/latest/posix_sync/mutex/)
carry the same example against a shared mapping, compiled wherever it applies.

## Modules

| Module    | Owned             | Borrowed                 | Notes                                                       |
|-----------|-------------------|--------------------------|-------------------------------------------------------------|
| `mutex`   | `OwnedMutex<R>`   | `BorrowedMutex<'a, R>`   | robustness, type, protocol, priority ceiling, timed locking |
| `condvar` | `OwnedCondvar`    | `BorrowedCondvar<'a>`    | selectable clock for timed waits                            |
| `rwlock`  | `OwnedRwLock`     | `BorrowedRwLock<'a>`     | reader/writer preference on glibc, timed locking            |
| `barrier` | `OwnedBarrier`    | `BorrowedBarrier<'a>`    | serial thread picked out of each released group             |

All four can be process-shared, as long as the platform supports it (see the table
below). They are non-poisoning, and therefore `!UnwindSafe` and `!RefUnwindSafe`.

## Platform support

|                          | Linux (glibc) | Linux (musl) | FreeBSD | DragonFly | NetBSD | OpenBSD | macOS/iOS | Android |
|--------------------------|---------------|--------------|---------|-----------|--------|---------|-----------|---------|
| Process sharing          | ✅            | ✅           | ✅      | ❌        | ❌     | ❌      | ✅        | ✅      |
| Robust mutexes           | ✅            | ✅           | ✅      | ❌        | ❌     | ❌      | ❌        | ❌      |
| Timed mutex locking      | ✅            | ✅           | ✅      | ✅        | ✅     | ✅      | ❌        | ✅      |
| Timed rwlock locking     | ✅            | ✅           | ✅      | ✅        | ✅     | ✅      | ❌        | ✅      |
| Condvar clock selection  | ✅            | ✅           | ✅      | ✅        | ✅     | ✅      | ❌        | ✅      |
| Priority inheritance     | ✅            | ✅           | ✅      | ✅        | ❌     | ✅      | ✅        | ✅      |
| Priority ceilings        | ✅            | ❌           | ✅      | ✅        | ✅     | ✅      | ✅        | ❌      |
| Reader/writer preference | ✅            | ❌           | ❌      | ❌        | ❌     | ❌      | ❌        | ❌      |
| Barriers                 | ✅            | ✅           | ✅      | ✅        | ✅     | ✅      | ❌        | ✅      |

- **Process sharing**: `with_sharing` and the `*Sharing` enums on all four builders.
  DragonFly, NetBSD and OpenBSD never implemented the option: OpenBSD's libraries export
  no pshared functions for mutexes or condvars at all, NetBSD's return `ENOSYS` for the
  shared value, and DragonFly's reject it with `EINVAL`. Where the row is ❌,
  `with_sharing` and the `*Sharing` enums do not exist, so the locks there can only
  synchronise threads within a single process, never two processes over shared memory.
- **Robust mutexes**: macOS/iOS, NetBSD, OpenBSD and Android do not implement robust
  mutexes at all. DragonFly declares the robust functions but does not define them.
- **Timed mutex locking**: macOS/iOS has no `pthread_mutex_timedlock`.
- **Timed rwlock locking**: macOS/iOS never implemented the timed rwlock functions.
- **Condvar clock selection**: macOS/iOS has no `pthread_condattr_setclock`, so a condvar
  there always measures its timed waits against `CLOCK_REALTIME`.
- **Priority inheritance**: NetBSD rejects `PTHREAD_PRIO_INHERIT` with `ENOTSUP` while
  accepting the other two protocols. On Android the protocol functions only exist from API
  level 28, so selecting any protocol there needs a target at least that new.
- **Priority ceilings**: the musl and Android C libraries leave the POSIX Thread Priority
  Protection option unimplemented: neither exports `pthread_mutexattr_setprioceiling`, and
  both reject `PTHREAD_PRIO_PROTECT`.
- **Reader/writer preference**: choosing who wins when readers and writers contend is a
  GNU extension (`pthread_rwlockattr_setkind_np`) rather than part of POSIX. Android's C
  library exports a variant with its own differently numbered constants, and FreeBSD
  declares the functions without defining them, so this crate offers the preference on
  glibc alone.
- **Barriers**: Apple platforms never implemented the POSIX Barriers option, so the
  `barrier` module does not exist there. On Android the barrier functions only exist from
  API level 24, so using the module there needs a target at least that new.

The crate checks against Rust 1.65.

## License

MIT. See [LICENSE](LICENSE).
