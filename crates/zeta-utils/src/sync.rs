//! Lock helper that recovers from poisoning.
//!
//! A panic in one task must not take command handling down, so the mutex recovers the (possibly
//! inconsistent) guard instead of propagating the panic.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks `mutex`, recovering from poisoning.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
