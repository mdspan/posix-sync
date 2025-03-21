#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use std::mem;
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use std::mem::{align_of, size_of};
use std::thread;
use std::time::Duration;
#[cfg(not(target_vendor = "apple"))]
use std::time::Instant;

mod utils;
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use utils::*;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use posix_sync::mutex::guards::{DataConsistency, RobustGuardContainer};
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use posix_sync::mutex::robustness_markers::Robust;
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use posix_sync::mutex::robustness_markers::RobustnessMarker;
use posix_sync::mutex::robustness_markers::Standard;
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use posix_sync::mutex::{BorrowedMutex, MutexSharing, RawMutexAlloc};
use posix_sync::mutex::{MutexBuilder, MutexLockError, MutexType, OwnedMutex};

/// Constructs a process-shared mutex of the requested robustness at the start of the mapping.
/// Only exists where the platform implements process sharing, like everything that uses it.
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
unsafe fn shared_mutex_from_mmap<R: RobustnessMarker>(mmap: &MmapRAII) -> BorrowedMutex<'_, R> {
    MutexBuilder::<R>::new()
        .with_sharing(MutexSharing::Shared)
        .build_borrowed(mmap.as_mut_ptr() as *mut RawMutexAlloc, mmap)
}

#[test]
fn owned_mutex_locks_and_unlocks() {
    let mtx = OwnedMutex::<Standard>::new();

    drop(mtx.lock().unwrap());
    assert!(mtx.try_lock().unwrap().is_some());

    let guard = mtx.lock().unwrap();
    thread::scope(|s| {
        s.spawn(|| assert!(mtx.try_lock().unwrap().is_none()));
    });
    drop(guard);

    assert!(mtx.try_lock().unwrap().is_some());
}

#[test]
fn owned_mutex_serializes_threads() {
    const THREADS: usize = 8;
    const INCREMENTS: usize = 2000;

    let mtx = OwnedMutex::<Standard>::new();
    let mut counter = 0usize;
    let cell = std::ptr::addr_of_mut!(counter) as usize;

    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for _ in 0..INCREMENTS {
                    let _guard = mtx.lock().unwrap();
                    // Deliberately not atomic: the mutex is what makes this well defined.
                    unsafe {
                        let cell = cell as *mut usize;
                        cell.write_volatile(cell.read_volatile() + 1);
                    }
                }
            });
        }
    });

    assert_eq!(counter, THREADS * INCREMENTS);
}

#[test]
fn error_checked_mutex_reports_deadlock() {
    let mtx = MutexBuilder::<Standard>::new()
        .with_type(MutexType::ErrorCheck)
        .build_owned();

    let _guard = mtx.lock().unwrap();
    assert_eq!(mtx.lock().unwrap_err(), MutexLockError::Deadlock);
}

#[test]
fn recursive_mutex_allows_relocking() {
    let mtx = MutexBuilder::<Standard>::new()
        .with_type(MutexType::Recursive)
        .build_owned();

    let outer = mtx.lock().unwrap();
    let inner = mtx.lock().unwrap();

    // Still held by the outer lock, so another thread gets nowhere.
    thread::scope(|s| {
        s.spawn(|| assert!(mtx.try_lock().unwrap().is_none()));
    });

    drop(inner);
    drop(outer);
    assert!(mtx.try_lock().unwrap().is_some());
}

#[cfg(not(target_vendor = "apple"))]
#[test]
fn lock_for_gives_up_at_the_deadline() {
    let mtx = OwnedMutex::<Standard>::new();
    let guard = mtx.lock().unwrap();

    thread::scope(|s| {
        s.spawn(|| {
            let start = Instant::now();
            assert!(mtx.lock_for(Duration::from_millis(150)).unwrap().is_none());
            assert!(start.elapsed() >= Duration::from_millis(140));
        });
    });

    drop(guard);
    assert!(mtx.lock_for(Duration::from_millis(150)).unwrap().is_some());
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[test]
fn robust_mutex_returns_correct_guards() -> Result<()> {
    let mmap = MmapRAII::new(RawMutexAlloc::SIZE);
    let mtx = unsafe { shared_mutex_from_mmap::<Robust>(&mmap) };

    // A child that dies holding the lock leaves it to the next owner to sort out.
    let pid = unsafe {
        in_child(|| {
            match mtx.lock()? {
                // Leaking the guard is how the child gets to die still holding the lock.
                RobustGuardContainer::Standard(guard) => {
                    mem::forget(guard);
                    Ok(())
                }
                RobustGuardContainer::Indeterminate(_) => Err(anyhow!("Unexpected guard.")),
            }
        })?
    };
    unsafe { wait_for_process(pid, None)? };

    match unsafe { mtx.lock()? } {
        RobustGuardContainer::Indeterminate(mut guard) => {
            guard.set_consistency_on_unlock(DataConsistency::Consistent);
        }
        RobustGuardContainer::Standard(_) => return Err(anyhow!("Unexpected guard.")),
    }

    // Now that the mutex has been marked consistent its behaviour is back to normal.
    assert!(!unsafe { mtx.lock()? }.is_indeterminate());

    // A child that unlocks before exiting is not an owner death at all.
    let pid = unsafe {
        in_child(|| {
            let guard = mtx.lock()?;
            drop(guard);
            Ok(())
        })?
    };
    unsafe { wait_for_process(pid, None)? };
    assert!(!unsafe { mtx.lock()? }.is_indeterminate());

    unsafe { mtx.destroy() };
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[test]
fn robust_mutex_recovers_through_make_consistent() -> Result<()> {
    let mmap = MmapRAII::new(RawMutexAlloc::SIZE);
    let mtx = unsafe { shared_mutex_from_mmap::<Robust>(&mmap) };

    let pid = unsafe {
        in_child(|| {
            mem::forget(mtx.lock()?);
            Ok(())
        })?
    };
    unsafe { wait_for_process(pid, None)? };

    let guard = unsafe { mtx.lock()? };
    assert!(guard.is_indeterminate());

    // Collapsing the container keeps the lock held the whole way through.
    let guard = guard.make_consistent()?;
    drop(guard);

    assert!(!unsafe { mtx.lock()? }.is_indeterminate());

    unsafe { mtx.destroy() };
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
#[test]
fn robust_mutex_non_recoverable_behavior() -> Result<()> {
    let mmap = MmapRAII::new(RawMutexAlloc::SIZE);
    let mtx = unsafe { shared_mutex_from_mmap::<Robust>(&mmap) };

    let pid = unsafe {
        in_child(|| {
            mem::forget(mtx.lock()?);
            Ok(())
        })?
    };
    unsafe { wait_for_process(pid, None)? };

    // Dropping the indeterminate guard without marking it consistent is what makes the mutex
    // unusable from here on.
    let guard = unsafe { mtx.lock()? };
    assert!(guard.is_indeterminate());
    drop(guard);

    let err = unsafe { mtx.lock() }.unwrap_err();
    assert_eq!(err, MutexLockError::NotRecoverable);

    unsafe { mtx.destroy() };
    Ok(())
}

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
#[test]
fn shared_mutex_serializes_processes() -> Result<()> {
    const PROCESSES: usize = 4;
    const INCREMENTS: usize = 2000;

    // The counter goes after the mutex, rounded up to its own alignment.
    let counter_offset = align_up(RawMutexAlloc::SIZE, align_of::<u64>());
    let mmap = MmapRAII::new(counter_offset + size_of::<u64>());

    let mtx = unsafe { shared_mutex_from_mmap::<Standard>(&mmap) };
    let counter = unsafe { mmap.offset_ptr(counter_offset) as *mut u64 };
    unsafe { counter.write_volatile(0) };

    let increment = move || {
        for _ in 0..INCREMENTS {
            let _guard = unsafe { mtx.lock()? };
            unsafe { counter.write_volatile(counter.read_volatile() + 1) };
        }
        Ok(())
    };

    let mut children = Vec::with_capacity(PROCESSES);
    for _ in 0..PROCESSES {
        children.push(unsafe { in_child(increment)? });
    }
    for pid in children {
        unsafe { wait_for_process(pid, Some(Duration::from_secs(10)))? };
    }

    assert_eq!(
        unsafe { counter.read_volatile() },
        (PROCESSES * INCREMENTS) as u64
    );

    unsafe { mtx.destroy() };
    Ok(())
}
