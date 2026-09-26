// Apple platforms have no barriers, so there is nothing here to test on them.
#![cfg(not(target_vendor = "apple"))]

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use std::mem::{align_of, size_of};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use std::time::Duration;

mod utils;
#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use utils::*;

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
use posix_sync::barrier::{BarrierBuilder, BarrierSharing, BorrowedBarrier, RawBarrierAlloc};
use posix_sync::barrier::{BarrierInitError, BarrierWaitOutcome, OwnedBarrier};

#[test]
fn nobody_is_released_before_everybody_arrives() {
    const THREADS: u32 = 8;

    let barrier = OwnedBarrier::new(THREADS).unwrap();
    let arrived = AtomicU32::new(0);

    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                arrived.fetch_add(1, Ordering::Relaxed);
                barrier.wait().unwrap();
                assert_eq!(arrived.load(Ordering::Relaxed), THREADS);
            });
        }
    });
}

#[test]
fn every_group_has_exactly_one_serial_thread() {
    const THREADS: u32 = 4;
    const ROUNDS: u32 = 200;

    let barrier = OwnedBarrier::new(THREADS).unwrap();
    let serial = AtomicU32::new(0);

    thread::scope(|s| {
        for _ in 0..THREADS {
            s.spawn(|| {
                for _ in 0..ROUNDS {
                    if barrier.wait().unwrap() == BarrierWaitOutcome::Serial {
                        serial.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
    });

    // The barrier resets after each group, so every round picks a serial thread of its own.
    assert_eq!(serial.load(Ordering::Relaxed), ROUNDS);
}

#[test]
fn a_count_of_one_never_blocks() {
    let barrier = OwnedBarrier::new(1).unwrap();
    assert_eq!(barrier.wait().unwrap(), BarrierWaitOutcome::Serial);
    assert_eq!(barrier.wait().unwrap(), BarrierWaitOutcome::Serial);
}

#[test]
fn a_count_of_zero_is_rejected() {
    assert_eq!(OwnedBarrier::new(0).unwrap_err(), BarrierInitError::Invalid);
}

#[cfg(not(any(target_os = "dragonfly", target_os = "netbsd", target_os = "openbsd")))]
#[test]
fn shared_barrier_synchronises_processes() -> Result<()> {
    const PROCESSES: u32 = 4;
    const ROUNDS: u32 = 100;

    let serial_offset = align_up(RawBarrierAlloc::SIZE, align_of::<AtomicU32>());
    let arrived_offset = serial_offset + size_of::<AtomicU32>();
    let mmap = MmapRAII::new(arrived_offset + size_of::<AtomicU32>());

    let barrier: BorrowedBarrier = unsafe {
        BarrierBuilder::new()
            .with_sharing(BarrierSharing::Shared)
            .build_borrowed(mmap.as_mut_ptr() as *mut RawBarrierAlloc, &mmap, PROCESSES)?
    };

    // The mapping starts out zeroed, which is a valid AtomicU32 for both counters.
    let serial = unsafe { &*(mmap.offset_ptr(serial_offset) as *const AtomicU32) };
    let arrived = unsafe { &*(mmap.offset_ptr(arrived_offset) as *const AtomicU32) };

    let work = move || {
        for round in 1..=ROUNDS {
            arrived.fetch_add(1, Ordering::Relaxed);
            if unsafe { barrier.wait()? } == BarrierWaitOutcome::Serial {
                serial.fetch_add(1, Ordering::Relaxed);
            }
            // Every process has arrived for this round, and none can have arrived for the next
            // one, because that takes this process as well.
            let seen = arrived.load(Ordering::Relaxed);
            if seen < round * PROCESSES || seen > round * PROCESSES + PROCESSES - 1 {
                return Err(anyhow!("round {round} saw {seen} arrivals"));
            }
        }
        Ok(())
    };

    let mut children = Vec::with_capacity(PROCESSES as usize);
    for _ in 0..PROCESSES {
        children.push(unsafe { in_child(work)? });
    }
    for pid in children {
        unsafe { wait_for_process(pid, Some(Duration::from_secs(10)))? };
    }

    assert_eq!(serial.load(Ordering::Relaxed), ROUNDS);

    unsafe { barrier.destroy() };
    Ok(())
}
