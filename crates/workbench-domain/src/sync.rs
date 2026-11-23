//! Poison-tolerant mutex locking shared across crates.
use std::sync::{Mutex, MutexGuard};

/// Acquires a mutex, recovering the inner value if a previous holder panicked.
pub fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
