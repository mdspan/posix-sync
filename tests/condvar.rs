#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use std::mem::{align_of, size_of};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

mod utils;
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use utils::*;

#[cfg(not(target_vendor = "apple"))]
use posix_sync::condvar::CondvarClock;
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use posix_sync::condvar::{BorrowedCondvar, CondvarSharing, RawCondvarAlloc};
use posix_sync::condvar::{CondvarBuilder, OwnedCondvar, WaitOutcome};
use posix_sync::mutex::robustness_markers::Standard;
use posix_sync::mutex::OwnedMutex;
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use posix_sync::mutex::{BorrowedMutex, MutexBuilder, MutexSharing, RawMutexAlloc};

/// How long a waiter is willing to sit there before the test decides the wakeup is never coming.
const PATIENCE: Duration = Duration::from_secs(5);

#[test]
fn notify_one_wakes_a_waiter() {
    let mtx = OwnedMutex::<Standard>::new();
    let cv = OwnedCondvar::new();
    let ready = AtomicBool::new(false);

    thread::scope(|s| {
        s.spawn(|| {
            let mut guard = mtx.lock().unwrap();
            while !ready.load(Ordering::Acquire) {
                // The untimed wait, which only ever returns once somebody notifies.
                cv.wait(&mut guard).unwrap();
            }
        });

        // Give the waiter time to actually get into the wait rather than skipping it.
        thread::sleep(Duration::from_millis(100));

        let guard = mtx.lock().unwrap();
        ready.store(true, Ordering::Release);
        drop(guard);
        cv.notify_one().unwrap();
    });
}

#[test]
fn notify_all_wakes_every_waiter() {
    const WAITERS: usize = 6;

    let mtx = OwnedMutex::<Standard>::new();
    let cv = OwnedCondvar::new();
    let ready = AtomicBool::new(false);
    let woken = AtomicUsize::new(0);

    thread::scope(|s| {
        for _ in 0..WAITERS {
            s.spawn(|| {
                let mut guard = mtx.lock().unwrap();
                while !ready.load(Ordering::Acquire) {
                    let outcome = cv.wait_for(&mut guard, PATIENCE).unwrap();
                    assert_eq!(outcome, WaitOutcome::Notified);
                }
                woken.fetch_add(1, Ordering::AcqRel);
            });
        }

        thread::sleep(Duration::from_millis(100));

        let guard = mtx.lock().unwrap();
        ready.store(true, Ordering::Release);
        drop(guard);
        cv.notify_all().unwrap();
    });

    assert_eq!(woken.load(Ordering::Acquire), WAITERS);
}

#[test]
fn wait_for_gives_up_at_the_deadline() {
    let mtx = OwnedMutex::<Standard>::new();
    let cv = OwnedCondvar::new();

    let mut guard = mtx.lock().unwrap();
    let start = Instant::now();
    let outcome = cv.wait_for(&mut guard, Duration::from_millis(150)).unwrap();

    assert_eq!(outcome, WaitOutcome::TimedOut);
    assert!(start.elapsed() >= Duration::from_millis(140));

    // The mutex is held again either way, so the guard is still good.
    drop(guard);
    assert!(mtx.try_lock().unwrap().is_some());
}

#[cfg(not(target_vendor = "apple"))]
#[test]
fn monotonic_condvar_resolves_deadlines_on_its_own_clock() {
    let mtx = OwnedMutex::<Standard>::new();
    let cv = CondvarBuilder::new()
        .with_clock(CondvarClock::Monotonic)
        .build_owned();

    assert_eq!(cv.clock(), CondvarClock::Monotonic);

    let mut guard = mtx.lock().unwrap();
    let start = Instant::now();
    let outcome = cv.wait_for(&mut guard, Duration::from_millis(150)).unwrap();

    assert_eq!(outcome, WaitOutcome::TimedOut);
    assert!(start.elapsed() >= Duration::from_millis(140));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
#[should_panic(expected = "different mutex")]
fn waiting_with_a_second_mutex_panics() {
    let first = OwnedMutex::<Standard>::new();
    let second = OwnedMutex::<Standard>::new();
    let cv = OwnedCondvar::new();

    let mut guard = first.lock().unwrap();
    cv.wait_for(&mut guard, Duration::from_millis(1)).unwrap();
    drop(guard);

    let mut guard = second.lock().unwrap();
    let _ = cv.wait_for(&mut guard, Duration::from_millis(1));
}

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
#[test]
fn shared_condvar_wakes_another_process() -> Result<()> {
    // mutex, then condvar, then the flag they are there to guard, all in one mapping.
    let cv_offset = align_up(RawMutexAlloc::SIZE, RawCondvarAlloc::ALIGN);
    let flag_offset = align_up(cv_offset + RawCondvarAlloc::SIZE, align_of::<u32>());
    let mmap = MmapRAII::new(flag_offset + size_of::<u32>());

    let mtx: BorrowedMutex<Standard> = unsafe {
        MutexBuilder::new()
            .with_sharing(MutexSharing::Shared)
            .build_borrowed(mmap.as_mut_ptr() as *mut RawMutexAlloc, &mmap)
    };
    let cv: BorrowedCondvar = unsafe {
        let builder = CondvarBuilder::new().with_sharing(CondvarSharing::Shared);
        // Darwin has no pthread_condattr_setclock, so there the condvar stays on CLOCK_REALTIME.
        #[cfg(not(target_vendor = "apple"))]
        let builder = builder.with_clock(CondvarClock::Monotonic);
        builder.build_borrowed(mmap.offset_ptr(cv_offset) as *mut RawCondvarAlloc, &mmap)
    };

    let flag = unsafe { mmap.offset_ptr(flag_offset) as *mut u32 };
    unsafe { flag.write_volatile(0) };

    let pid = unsafe {
        in_child(|| {
            let mut guard = mtx.lock()?;
            while flag.read_volatile() == 0 {
                match cv.wait_for(&mut guard, PATIENCE)? {
                    WaitOutcome::Notified => {}
                    WaitOutcome::TimedOut => return Err(anyhow!("the parent never showed up")),
                }
            }
            // Dropped here rather than left to _exit, which does not run destructors.
            drop(guard);
            Ok(())
        })?
    };

    thread::sleep(Duration::from_millis(100));

    unsafe {
        let guard = mtx.lock()?;
        flag.write_volatile(1);
        drop(guard);
        cv.notify_all()?;
    }

    unsafe { wait_for_process(pid, Some(Duration::from_secs(10)))? };

    unsafe {
        cv.destroy();
        mtx.destroy();
    }
    Ok(())
}
