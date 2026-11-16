//! The single-slot [`TtlCache`]: one value, refreshed lazily and kept until its TTL expires.

use std::fmt;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwapOption;

use crate::entry::Entry;

/// A single-slot cache holding a value until its time-to-live expires.
///
/// Reads of a fresh value are synchronous; refreshing requires `async` and is expected to be
/// driven by the owner (e.g. when handling a command). A stale or missing value is only
/// replaced after a successful refresh — failures leave the existing entry in place, and
/// readers keep seeing it until a refresh succeeds.
pub struct TtlCache<T> {
    /// The cached value and its expiry time, if populated.
    entry: ArcSwapOption<Entry<T>>,
    /// How long a cached value stays fresh.
    ttl: Duration,
    /// The single-flight lock: concurrent misses refresh once, the tasks that wait for the
    /// lock pick up the value the first refresh produced.
    refreshing: tokio::sync::Mutex<()>,
}

impl<T> TtlCache<T> {
    /// Creates an empty cache where cached values expire after `ttl`.
    #[must_use]
    pub const fn new(ttl: Duration) -> Self {
        Self {
            entry: ArcSwapOption::const_empty(),
            ttl,
            refreshing: tokio::sync::Mutex::const_new(()),
        }
    }

    /// Creates a cache pre-populated with `value`, expiring after `ttl`.
    #[must_use]
    pub fn new_with_value(value: T, ttl: Duration) -> Self {
        let cache = Self::new(ttl);

        cache.replace(Entry::new(value, ttl));

        cache
    }

    /// Runs `access` with the cached value if one exists, fresh or stale.
    ///
    /// Stale values are served so callers that pair this with a preceding refresh attempt keep
    /// working with the last known data while the source is unavailable.
    #[must_use]
    pub fn with_value<R>(&self, access: impl FnOnce(Option<&T>) -> R) -> R {
        let entry = self.entry.load();

        access(entry.as_ref().map(|entry| &entry.value))
    }

    /// Returns whether the cache holds a value that has not expired.
    fn is_fresh(&self) -> bool {
        let entry = self.entry.load();

        entry.as_ref().is_some_and(|entry| entry.is_fresh())
    }

    /// Returns a clone of the cached value when it is fresh.
    ///
    /// Unlike [`TtlCache::with_value`], a stale value is not returned.
    #[must_use]
    #[allow(clippy::redundant_closure_for_method_calls)] // a bare `Option::cloned` is ambiguous
    pub fn get(&self) -> Option<T>
    where
        T: Clone,
    {
        let entry = self.entry.load();

        entry
            .as_ref()
            .filter(|entry| entry.is_fresh())
            .map(|entry| entry.value.clone())
    }

    /// Returns the cached value when it is fresh, refreshing through `refresh` otherwise.
    ///
    /// Unlike [`TtlCache::with_value`], a stale value is not returned; a missing or expired value
    /// triggers a refresh, whose result is cached and returned. Failures are returned without
    /// caching anything, leaving any stale value in place.
    ///
    /// Concurrent misses refresh once: the first task refreshes while the others wait, and
    /// pick up the refreshed value through the double-check afterwards.
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
        let ttl = self.ttl;

        self.get_or_refresh_with(|| async move {
            let value = refresh().await?;

            Ok((value, ttl))
        })
        .await
    }

    /// Returns the cached value when it is fresh, refreshing through `refresh` otherwise and
    /// caching the refreshed value for the [`Duration`] it provides.
    ///
    /// Unlike [`TtlCache::get_or_refresh`], the refresh chooses how long its value stays fresh,
    /// for callers whose entries carry a lifetime of their own — e.g. an OAuth2 token that the
    /// authorization server expires on its own schedule. A zero [`Duration`] makes the value
    /// immediately stale, so the next read refreshes again.
    ///
    /// Concurrent misses refresh once: the first task refreshes while the others wait, and
    /// pick up the refreshed value through the double-check afterwards.
    ///
    /// # Errors
    ///
    /// Returns the error produced by `refresh` when the value could not be refreshed.
    pub async fn get_or_refresh_with<E, F, Fut>(&self, refresh: F) -> Result<T, E>
    where
        T: Clone + Send + Sync,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<(T, Duration), E>> + Send,
    {
        if let Some(value) = self.get() {
            return Ok(value);
        }

        let _flight = self.refreshing.lock().await;

        if let Some(value) = self.get() {
            return Ok(value);
        }

        let (value, ttl) = refresh().await?;

        self.replace(Entry::new(value.clone(), ttl));

        Ok(value)
    }

    /// Populates the cache through `refresh` when its value is missing or expired.
    ///
    /// Unlike [`TtlCache::get_or_refresh`], the refreshed value is not returned and `T` needs
    /// no `Clone`; readers access it through [`TtlCache::with_value`] or [`TtlCache::get`].
    ///
    /// Concurrent refresh attempts are single-flight: the first refreshes while the others
    /// wait and return once it has populated the cache.
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

        let _flight = self.refreshing.lock().await;

        if self.is_fresh() {
            return Ok(());
        }

        let value = refresh().await?;
        self.replace(Entry::new(value, self.ttl));

        Ok(())
    }

    /// Refreshes the cache through `refresh` regardless of its current freshness, caching and
    /// returning the refreshed value.
    ///
    /// Unlike [`TtlCache::get_or_refresh`], the refresh is unconditional — for callers that
    /// know the cached value is wrong (e.g. a credential that the server just rejected).
    /// Concurrent force-refreshes are serialized.
    ///
    /// # Errors
    ///
    /// Returns the error produced by `refresh` when the value could not be refreshed.
    pub async fn force_refresh<E, F, Fut>(&self, refresh: F) -> Result<T, E>
    where
        T: Clone + Send + Sync,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<T, E>> + Send,
    {
        let _flight = self.refreshing.lock().await;

        let value = refresh().await?;
        self.replace(Entry::new(value.clone(), self.ttl));

        Ok(value)
    }

    /// Replaces the cached entry.
    fn replace(&self, entry: Entry<T>) {
        self.entry.store(Some(Arc::new(entry)));
    }
}

impl<T> fmt::Debug for TtlCache<T>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let entry = self.entry.load();

        f.debug_struct("TtlCache")
            .field("entry", &entry.as_deref())
            .field("ttl", &self.ttl)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
