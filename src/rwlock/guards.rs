//! RAII guards returned from successful lock operations.

use std::fmt::{self, Debug};
use std::marker::PhantomData;

use libc::pthread_rwlock_t;

use super::errors::RwLockError;
use crate::utils::{AsRawUnderlying, Sealed};

/// Unlocks the rwlock behind `raw`. Both guards go through the same libc call: `pthread_rwlock_unlock`
/// releases whichever kind of lock the calling thread happens to hold.
#[inline]
unsafe fn unlock(raw: *mut pthread_rwlock_t) {
    let r = libc::pthread_rwlock_unlock(raw);
    debug_assert_eq!(r, 0);
}

/// A RAII guard returned by a successful read lock. Any number of these can exist at once, across
/// any number of threads and processes.
pub struct ReadGuard<'a> {
    pub(super) raw: *mut pthread_rwlock_t,
    pub(super) _phantom: PhantomData<&'a ()>,
}

unsafe impl Sync for ReadGuard<'_> {}
impl Sealed for ReadGuard<'_> {}

impl AsRawUnderlying for ReadGuard<'_> {
    type Underlying = pthread_rwlock_t;

    fn as_raw_underlying(&self) -> *mut pthread_rwlock_t {
        self.raw
    }
}

impl Debug for ReadGuard<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadGuard").finish_non_exhaustive()
    }
}

impl Drop for ReadGuard<'_> {
    /// Releases the read lock.
    #[inline]
    fn drop(&mut self) {
        unsafe { unlock(self.raw) };
    }
}

/// A RAII guard returned by a successful write lock. While one of these exists there are no other
/// holders of the lock, readers or writers.
pub struct WriteGuard<'a> {
    pub(super) raw: *mut pthread_rwlock_t,
    pub(super) _phantom: PhantomData<&'a ()>,
}

unsafe impl Sync for WriteGuard<'_> {}
impl Sealed for WriteGuard<'_> {}

impl AsRawUnderlying for WriteGuard<'_> {
    type Underlying = pthread_rwlock_t;

    fn as_raw_underlying(&self) -> *mut pthread_rwlock_t {
        self.raw
    }
}

impl Debug for WriteGuard<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteGuard").finish_non_exhaustive()
    }
}

impl Drop for WriteGuard<'_> {
    /// Releases the write lock.
    #[inline]
    fn drop(&mut self) {
        unsafe { unlock(self.raw) };
    }
}

/// Constructs a read guard from a rwlock and a libc return value.
pub(super) fn read_guard_from_libc_returnval<L>(
    lock: &L,
    r: i32,
) -> Result<ReadGuard<'_>, RwLockError>
where
    L: AsRawUnderlying<Underlying = pthread_rwlock_t>,
{
    match r {
        0 => Ok(ReadGuard {
            raw: lock.as_raw_underlying(),
            _phantom: PhantomData,
        }),
        e => Err(RwLockError::from(e)),
    }
}

/// Constructs a write guard from a rwlock and a libc return value.
pub(super) fn write_guard_from_libc_returnval<L>(
    lock: &L,
    r: i32,
) -> Result<WriteGuard<'_>, RwLockError>
where
    L: AsRawUnderlying<Underlying = pthread_rwlock_t>,
{
    match r {
        0 => Ok(WriteGuard {
            raw: lock.as_raw_underlying(),
            _phantom: PhantomData,
        }),
        e => Err(RwLockError::from(e)),
    }
}
