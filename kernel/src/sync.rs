//! Counted kernel locks (V0.10, SYNC10-001).
//!
//! V0.10 runs background processes from bounded slices at a few audited
//! "safe points" in kernel code (ADR-0022). A slice runs Ring 3 code and the
//! kernel paths it calls, so it must never start while the kernel itself
//! holds a lock: the slice could need the same lock, and the spin locks here
//! do not mask interrupts or detect self-deadlock. This wrapper counts every
//! lock held on the (single) CPU; a safe point refuses to slice unless the
//! count is zero. `clippy.toml` forbids the raw `spin` mutex everywhere else,
//! so a new lock cannot silently escape the count.
//!
//! The kernel heap's `LockedHeap` is deliberately not wrapped: allocator
//! internals never reach a safe point, so it cannot be held at one.

// This module is the one place allowed to name the raw type.
#![allow(clippy::disallowed_types)]

use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Locks currently held (main context plus any interrupt handler in flight).
static DEPTH: AtomicUsize = AtomicUsize::new(0);

/// How many counted locks are held right now.
pub fn depth() -> usize {
    DEPTH.load(Ordering::Relaxed)
}

/// A spin mutex that counts itself while held.
pub struct Mutex<T: ?Sized>(spin::Mutex<T>);

impl<T> Mutex<T> {
    pub const fn new(value: T) -> Self {
        Mutex(spin::Mutex::new(value))
    }
}

impl<T: ?Sized> Mutex<T> {
    pub fn lock(&self) -> MutexGuard<'_, T> {
        let guard = self.0.lock();
        DEPTH.fetch_add(1, Ordering::Relaxed);
        MutexGuard(guard)
    }

    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        let guard = self.0.try_lock()?;
        DEPTH.fetch_add(1, Ordering::Relaxed);
        Some(MutexGuard(guard))
    }
}

/// The guard; dropping it releases the lock and the count.
pub struct MutexGuard<'a, T: ?Sized>(spin::MutexGuard<'a, T>);

impl<T: ?Sized> Deref for MutexGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: ?Sized> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        DEPTH.fetch_sub(1, Ordering::Relaxed);
    }
}
