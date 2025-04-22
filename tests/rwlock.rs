#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use std::mem::{align_of, size_of};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;
#[cfg(not(target_vendor = "apple"))]
use std::time::Instant;

mod utils;
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use utils::*;

use posix_sync::rwlock::OwnedRwLock;
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use posix_sync::rwlock::{BorrowedRwLock, RawRwLockAlloc, RwLockBuilder, RwLockSharing};

#[test]
fn readers_do_not_exclude_each_other() {
    const READERS: usize = 8;

    let lock = OwnedRwLock::new();
    let holding = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);

    thread::scope(|s| {
        for _ in 0..READERS {
            s.spawn(|| {
                let _guard = lock.read().unwrap();

                let now = holding.fetch_add(1, Ordering::AcqRel) + 1;
                peak.fetch_max(now, Ordering::AcqRel);

                // Hold on long enough that the other readers get a chance to pile in.
                thread::sleep(Duration::from_millis(100));
                holding.fetch_sub(1, Ordering::AcqRel);
            });
        }
    });

    assert!(peak.load(Ordering::Acquire) > 1, "readers were serialized");
}

#[test]
fn a_writer_excludes_everyone() {
    let lock = OwnedRwLock::new();
    let guard = lock.write().unwrap();

    thread::scope(|s| {
        s.spawn(|| {
            assert!(lock.try_read().unwrap().is_none());
            assert!(lock.try_write().unwrap().is_none());
        });
    });

    drop(guard);
    assert!(lock.try_write().unwrap().is_some());
}

#[test]
fn a_reader_excludes_writers() {
    let lock = OwnedRwLock::new();
    let guard = lock.read().unwrap();

    thread::scope(|s| {
        s.spawn(|| {
            assert!(lock.try_read().unwrap().is_some());
            assert!(lock.try_write().unwrap().is_none());
        });
    });

    drop(guard);
    assert!(lock.try_write().unwrap().is_some());
}

#[cfg(not(target_vendor = "apple"))]
#[test]
fn timed_locks_give_up_at_the_deadline() {
    let lock = OwnedRwLock::new();
    let guard = lock.write().unwrap();

    thread::scope(|s| {
        s.spawn(|| {
            let start = Instant::now();
            assert!(lock.read_for(Duration::from_millis(150)).unwrap().is_none());
            assert!(start.elapsed() >= Duration::from_millis(140));

            let start = Instant::now();
            assert!(lock
                .write_for(Duration::from_millis(150))
                .unwrap()
                .is_none());
            assert!(start.elapsed() >= Duration::from_millis(140));
        });
    });

    drop(guard);
    assert!(lock.read_for(Duration::from_millis(150)).unwrap().is_some());
    assert!(lock
        .write_for(Duration::from_millis(150))
        .unwrap()
        .is_some());
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test]
fn a_writer_preferring_lock_blocks_later_readers() {
    use posix_sync::rwlock::RwLockPreference;

    let lock = RwLockBuilder::new()
        .with_preference(RwLockPreference::WriterNonRecursive)
        .build_owned();

    let first_reader = lock.read().unwrap();

    thread::scope(|s| {
        // A writer that queues up behind the reader currently holding the lock.
        let writer = s.spawn(|| {
            let _guard = lock.write().unwrap();
        });

        thread::sleep(Duration::from_millis(100));

        // With writer preference this reader has to wait for the queued writer, even though the
        // lock is only read locked right now. A reader-preferring lock would let it straight in.
        assert!(lock.try_read().unwrap().is_none());

        drop(first_reader);
        writer.join().unwrap();
    });

    assert!(lock.try_write().unwrap().is_some());
}

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
#[test]
fn shared_rwlock_excludes_another_process() -> Result<()> {
    const PROCESSES: usize = 4;
    const INCREMENTS: usize = 1000;

    let counter_offset = align_up(RawRwLockAlloc::SIZE, align_of::<u64>());
    let mmap = MmapRAII::new(counter_offset + size_of::<u64>());

    let lock: BorrowedRwLock = unsafe {
        RwLockBuilder::new()
            .with_sharing(RwLockSharing::Shared)
            .build_borrowed(mmap.as_mut_ptr() as *mut RawRwLockAlloc, &mmap)
    };

    let counter = unsafe { mmap.offset_ptr(counter_offset) as *mut u64 };
    unsafe { counter.write_volatile(0) };

    let work = move || {
        for _ in 0..INCREMENTS {
            {
                let _guard = unsafe { lock.write()? };
                unsafe { counter.write_volatile(counter.read_volatile() + 1) };
            }
            // Readers of a half-written counter would be the thing to catch here, so take the
            // read lock too and check the value is one the writers could have left behind.
            let _guard = unsafe { lock.read()? };
            assert!(unsafe { counter.read_volatile() } <= (PROCESSES * INCREMENTS) as u64);
        }
        Ok(())
    };

    let mut children = Vec::with_capacity(PROCESSES);
    for _ in 0..PROCESSES {
        children.push(unsafe { in_child(work)? });
    }
    for pid in children {
        unsafe { wait_for_process(pid, Some(Duration::from_secs(10)))? };
    }

    assert_eq!(
        unsafe { counter.read_volatile() },
        (PROCESSES * INCREMENTS) as u64
    );

    unsafe { lock.destroy() };
    Ok(())
}
