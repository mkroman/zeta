//! A single-slot cache whose entries expire after a time-to-live.
//!
//! Shared by plugins that cache an API response (e.g. currency lists or video categories) and
//! refresh it lazily after a configured TTL. Reads do not need to await, matching the pattern
//! of command handlers that peek at the cache synchronously while refreshes happen in `async`
//! code.

use std::future::Future;
use std::sync::{PoisonError, RwLock, RwLockReadGuard};
use std::time::{Duration, Instant};

/// A single-slot cache holding a value until its time-to-live expires.
///
/// Reads of a fresh value are synchronous; refreshing requires `async` and is expected to be
/// driven by the owner (e.g. when handling a command). A stale or missing value is only
/// replaced after a successful refresh — failures leave the existing entry in place, and
/// readers keep seeing it until a refresh succeeds.
pub struct TtlCache<T> {
    /// The cached value and its expiry time, if populated.
    entry: RwLock<Option<Entry<T>>>,
    /// How long a cached value stays fresh.
    ttl: Duration,
}

/// A cached value with its absolute expiry time.
struct Entry<T> {
    value: T,
    expires_at: Instant,
}

impl<T> TtlCache<T> {
    /// Creates an empty cache where cached values expire after `ttl`.
    #[must_use]
    pub const fn new(ttl: Duration) -> Self {
        Self {
            entry: RwLock::new(None),
            ttl,
        }
    }

    /// Creates a cache pre-populated with `value`, expiring after `ttl`.
    #[cfg(test)]
    pub(crate) fn with_value(value: T, ttl: Duration) -> Self {
        Self {
            entry: RwLock::new(Some(Entry {
                value,
                expires_at: Instant::now() + ttl,
            })),
            ttl,
        }
    }

    /// Runs `read` with the cached value if one exists, fresh or stale.
    ///
    /// Stale values are served so callers that pair this with a preceding refresh attempt keep
    /// working with the last known data while the source is unavailable.
    ///
    /// Poisoned locks are recovered from, mirroring how plugins read their caches: a panic in
    /// another task must not take the command handlers down.
    pub fn read<R>(&self, read: impl FnOnce(Option<&T>) -> R) -> R {
        let guard: RwLockReadGuard<'_, Option<Entry<T>>> =
            self.entry.read().unwrap_or_else(PoisonError::into_inner);

        read(guard.as_ref().map(|entry| &entry.value))
    }

    /// Returns whether the cache holds a value that has not expired.
    fn is_fresh(&self) -> bool {
        let guard = self.entry.read().unwrap_or_else(PoisonError::into_inner);

        guard
            .as_ref()
            .is_some_and(|entry| entry.expires_at > Instant::now())
    }

    /// Returns a clone of the cached value when it is fresh.
    ///
    /// Unlike [`TtlCache::read`], a stale value is not returned.
    #[must_use]
    #[allow(clippy::redundant_closure_for_method_calls)] // a bare `Option::cloned` is ambiguous
    pub fn get(&self) -> Option<T>
    where
        T: Clone,
    {
        let guard = self.entry.read().unwrap_or_else(PoisonError::into_inner);

        guard
            .as_ref()
            .filter(|entry| entry.expires_at > Instant::now())
            .map(|entry| entry.value.clone())
    }

    /// Returns the cached value when it is fresh, refreshing through `refresh` otherwise.
    ///
    /// Unlike [`TtlCache::read`], a stale value is not returned; a missing or expired value
    /// triggers a refresh, whose result is cached and returned. Failures are returned without
    /// caching anything, leaving any stale value in place.
    ///
    /// # Errors
    ///
    /// Returns the error produced by `refresh` when the value could not be refreshed.
    pub async fn get_or_refresh<E, F, Fut>(&self, refresh: F) -> Result<T, E>
    where
        T: Clone + Send + Sync,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<T, E>> + Send,
    {
        if let Some(value) = self.get() {
            return Ok(value);
        }

        let value = refresh().await?;
        let expires_at = Instant::now() + self.ttl;

        self.replace(Entry {
            value: value.clone(),
            expires_at,
        });

        Ok(value)
    }

    /// Populates the cache through `refresh` when its value is missing or expired.
    ///
    /// Unlike [`TtlCache::get_or_refresh`], the refreshed value is not returned and `T` needs
    /// no `Clone`; readers access it through [`TtlCache::read`] or [`TtlCache::get`].
    ///
    /// # Errors
    ///
    /// Returns the error produced by `refresh` when the value could not be refreshed.
    pub async fn refresh<E, F, Fut>(&self, refresh: F) -> Result<(), E>
    where
        T: Send + Sync,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<T, E>> + Send,
    {
        if self.is_fresh() {
            return Ok(());
        }

        let value = refresh().await?;
        self.replace(Entry {
            value,
            expires_at: Instant::now() + self.ttl,
        });

        Ok(())
    }

    /// Replaces the cached entry, tolerating a poisoned lock.
    fn replace(&self, entry: Entry<T>) {
        *self.entry.write().unwrap_or_else(PoisonError::into_inner) = Some(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::redundant_closure_for_method_calls)] // a bare `Option::copied` is ambiguous
    fn cached_value(cache: &TtlCache<u8>) -> Option<u8> {
        cache.read(|value| value.copied())
    }

    fn cache_with(ttl: Duration) -> TtlCache<u8> {
        TtlCache::new(ttl)
    }

    #[tokio::test]
    async fn returns_none_before_the_first_refresh() {
        let cache = cache_with(Duration::from_mins(1));

        assert_eq!(cached_value(&cache), None);
    }

    #[tokio::test]
    async fn get_or_refresh_populates_and_reuses_the_value() {
        let cache = cache_with(Duration::from_mins(1));
        let mut refreshes = 0;

        let first = cache
            .get_or_refresh(|| async {
                refreshes += 1;
                Ok::<_, ()>(7)
            })
            .await
            .unwrap();
        let second = cache
            .get_or_refresh(|| async {
                refreshes += 1;
                Ok::<_, ()>(9)
            })
            .await
            .unwrap();

        assert_eq!(first, 7);
        assert_eq!(second, 7);
        assert_eq!(refreshes, 1);
        assert_eq!(cache.get(), Some(7));
    }

    #[tokio::test]
    async fn get_or_refresh_surfacing_errors_without_caching() {
        let cache = cache_with(Duration::from_mins(1));

        let error = cache
            .get_or_refresh(|| async { Err("denied") })
            .await
            .unwrap_err();
        assert_eq!(error, "denied");
        assert_eq!(cache.get(), None);
    }

    #[tokio::test]
    async fn expires_after_the_ttl() {
        let cache = TtlCache::new(Duration::ZERO);
        *cache.entry.write().unwrap() = Some(Entry {
            value: 7,
            expires_at: Instant::now(),
        });

        // A zero TTL is never fresh, so `get` no longer sees the value...
        assert_eq!(cache.get(), None);
        // ...but `read` still serves it, and a refresh replaces it (though a zero TTL makes
        // the replacement immediately stale again).
        assert_eq!(cached_value(&cache), Some(7));
        cache
            .get_or_refresh(|| async { Ok::<_, ()>(9) })
            .await
            .unwrap();
        assert_eq!(cache.get(), None);
        assert_eq!(cached_value(&cache), Some(9));
    }

    #[tokio::test]
    async fn read_serves_the_stale_value_after_a_failed_refresh() {
        let cache = cache_with(Duration::from_mins(1));
        cache.get_or_refresh(|| async { Ok::<_, ()>(7) }).await.unwrap();

        // Expire the entry directly, so the test does not depend on wall-clock timing.
        cache.entry.write().unwrap().as_mut().unwrap().expires_at = Instant::now();

        let error = cache
            .get_or_refresh(|| async { Err("denied") })
            .await
            .unwrap_err();

        assert_eq!(error, "denied");
        assert_eq!(cache.get(), None);
        assert_eq!(cached_value(&cache), Some(7));
    }

    #[tokio::test]
    async fn read_sees_the_fresh_value() {
        let cache = cache_with(Duration::from_mins(1));

        assert_eq!(cached_value(&cache), None);

        cache.get_or_refresh(|| async { Ok::<_, ()>(7) }).await.unwrap();
        assert_eq!(cached_value(&cache), Some(7));
    }

    #[tokio::test]
    async fn refresh_populates_without_returning_or_cloning() {
        let cache = TtlCache::new(Duration::from_mins(1));
        let mut refreshes = 0;

        cache
            .refresh(|| async {
                refreshes += 1;
                Ok::<_, ()>(7)
            })
            .await
            .unwrap();
        cache
            .refresh(|| async {
                refreshes += 1;
                Ok::<_, ()>(9)
            })
            .await
            .unwrap();

        assert_eq!(refreshes, 1);
        assert_eq!(cached_value(&cache), Some(7));
        assert_eq!(cache.get(), Some(7));
    }
}
