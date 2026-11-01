//! Lock helpers that recover from poisoning.
//!
//! A panic in one task must not take command handling down, so the guards recover the (possibly
//! inconsistent) value instead of propagating the panic.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Locks `mutex`, recovering from poisoning.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Acquires a read guard on `lock`, recovering from poisoning.
pub fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

/// Acquires a write guard on `lock`, recovering from poisoning.
pub fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guards_recover_from_poisoning() {
        let mutex = Mutex::new(1);
        let rw_lock = RwLock::new(2);

        {
            let _guard = lock(&mutex);
        }
        {
            let _guard = read(&rw_lock);
        }
        {
            let _guard = write(&rw_lock);
        }

        let panic = std::panic::catch_unwind(|| {
            let _guard = mutex.lock().expect("the test mutex is unpoisoned");
            panic!("poison the mutex");
        });
        assert!(panic.is_err());

        assert_eq!(*lock(&mutex), 1);
        assert_eq!(*read(&rw_lock), 2);
    }
}
