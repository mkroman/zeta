//! The cached value and its absolute expiry, shared by both cache types.

use std::time::{Duration, Instant};

/// A cached value with its absolute expiry time.
///
/// [`TtlCache`](crate::TtlCache) stores its value directly; [`TtlMap`](crate::TtlMap) stores an
/// `Option` so the same entry type represents positive and negative results.
#[derive(Clone, Debug)]
pub(crate) struct Entry<T> {
    pub(crate) value: T,
    pub(crate) expires_at: Instant,
}

impl<T> Entry<T> {
    /// Wraps `value` with an expiry `ttl` from now.
    pub(crate) fn new(value: T, ttl: Duration) -> Self {
        Self {
            value,
            expires_at: Instant::now() + ttl,
        }
    }

    /// Returns whether the entry has not expired yet.
    pub(crate) fn is_fresh(&self) -> bool {
        self.expires_at > Instant::now()
    }
}
