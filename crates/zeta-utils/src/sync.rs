//! Lock helpers that recover from poisoning.
//!
//! A panic in one task must not take command handling down, so every standard lock recovers the
//! (possibly inconsistent) guard instead of propagating the panic.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Reads through `lock`, recovering from poisoning.
pub fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// Writes through `lock`, recovering from poisoning.
pub fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

/// Locks `mutex`, recovering from poisoning.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
