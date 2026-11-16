//! The keyed [`TtlMap`]: per-key expiry, negative caching, single-flight refreshes and bounded
//! eviction.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};

use crate::entry::Entry;

/// The default number of entries a [`TtlMap`] holds before evicting.
const DEFAULT_TTL_MAP_CAPACITY: usize = 256;

/// The number of concurrent refreshes a [`TtlMap`] preallocates room for.
const DEFAULT_FLIGHT_CAPACITY: usize = 16;

/// A keyed cache whose entries expire individually after a time-to-live.
///
/// Entries cache both values and *negative* results: a refresh that produces no value stores a
/// negative entry so a burst of requests for the same missing resource does not each hit the
/// source. Cached values are returned only while fresh — unlike
/// [`TtlCache`](crate::TtlCache), stale entries are never served.
pub struct TtlMap<K, V> {
    /// The cached entries, keyed by their lookup key.
    ///
    /// A short write guard covers an insert; reads share a read guard. These critical sections do
    /// run user code: map operations call `K`'s `Hash`/`Eq`, eviction clones a key, and replacing
    /// or evicting an entry drops a `V`. The lock is still safe without poisoning because
    /// `HashMap` is panic-safe and the guard is released while unwinding — a panic can leave an
    /// eviction partly done or skip the insert that triggered it, but never leaves the map
    /// structurally invalid.
    entries: RwLock<HashMap<K, Entry<Option<Arc<V>>>>>,
    /// Per-key single-flight locks: concurrent misses for the same key refresh once. Entries
    /// are removed once they are uncontended, so the map does not grow with refreshed keys.
    ///
    /// The critical sections clone and hash the key, so they do run `K`'s `Clone`/`Hash`/`Eq`
    /// implementations. The lock is still safe without poisoning: `HashMap` is panic-safe and the
    /// guard is released while unwinding, leaving the map structurally valid (at worst, an entry
    /// is not inserted or removed).
    flights: Mutex<HashMap<K, Arc<tokio::sync::Mutex<()>>>>,
    /// How long a cached value stays fresh.
    ttl: Duration,
    /// How long a cached negative result stays fresh.
    negative_ttl: Duration,
    /// The maximum number of entries held before the closest-to-expiring ones are evicted.
    capacity: usize,
}

impl<K, V> TtlMap<K, V>
where
    K: Eq + Hash + Clone,
{
    /// Creates an empty cache where cached values expire after `ttl` and negative results after
    /// `negative_ttl`, holding at most `capacity` entries.
    #[must_use]
    pub fn new_with_negative_ttl(ttl: Duration, negative_ttl: Duration, capacity: usize) -> Self {
        Self {
            entries: RwLock::new(HashMap::with_capacity(capacity)),
            flights: Mutex::new(HashMap::with_capacity(
                capacity.min(DEFAULT_FLIGHT_CAPACITY),
            )),
            ttl,
            negative_ttl,
            capacity,
        }
    }

    /// Creates an empty cache where cached values expire after `ttl` and negative results expire
    /// after a tenth of it, holding up to the default number of entries.
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        Self::new_with_negative_ttl(ttl, ttl / 10, DEFAULT_TTL_MAP_CAPACITY)
    }

    /// Returns the cached value for `key` when it has not expired.
    #[must_use]
    pub fn get(&self, key: &K) -> Option<V>
    where
        V: Clone,
    {
        self.peek(key).flatten()
    }

    /// Returns the cached entry for `key` when it has not expired, including negative results.
    ///
    /// `Some(None)` is a cached negative result; `None` means nothing is cached for `key`.
    #[must_use]
    pub fn peek(&self, key: &K) -> Option<Option<V>>
    where
        V: Clone,
    {
        self.peek_arc(key)
            .map(|entry| entry.map(|value| (*value).clone()))
    }

    /// Returns the cached entry for `key` when it has not expired, as a shared value.
    ///
    /// `Some(None)` is a cached negative result; `None` means nothing is cached for `key`.
    /// Only the [`Arc`] is cloned while the read guard is held, so cloning `V` happens after
    /// the guard is released.
    #[allow(clippy::option_option)] // the outer option separates a miss from a negative entry
    fn peek_arc(&self, key: &K) -> Option<Option<Arc<V>>> {
        let entries = self.entries.read();

        entries
            .get(key)
            .filter(|entry| entry.is_fresh())
            .map(|entry| entry.value.clone())
    }

    /// Returns the cached entry for `key` when fresh, refreshing through `refresh` otherwise.
    ///
    /// A `Some` value is cached for the full TTL; a `None` (a negative result) is cached for the
    /// negative TTL, so repeated misses for the same key do not each hit the source.
    ///
    /// Concurrent misses for the same key refresh once: the first task refreshes while the
    /// others wait, and pick up the refreshed entry through the double-check afterwards.
    ///
    /// # Errors
    ///
    /// Returns the error produced by `refresh` when the value could not be refreshed. Nothing is
    /// cached in that case.
    pub async fn get_or_refresh<E, F, Fut>(&self, key: K, refresh: F) -> Result<Option<V>, E>
    where
        V: Clone + Send + Sync,
        K: Send + Sync,
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Option<V>, E>> + Send,
    {
        if let Some(cached) = self.peek_arc(&key) {
            return Ok(cached.map(|value| (*value).clone()));
        }

        let flight = {
            let mut flights = self.flights.lock();

            Arc::clone(flights.entry(key.clone()).or_default())
        };
        // Declared before the guard so it outlives it: released on every exit, including
        // cancellation and a panic in `refresh`.
        let _release = FlightRelease {
            flights: &self.flights,
            key: &key,
        };
        let _flight_guard = flight.lock().await;

        // No `?` before the guard is dropped: every exit — including a failing refresh and the
        // double-check — has to give the flight entry a chance to be cleaned up.
        match self.peek_arc(&key) {
            // Another task refreshed the key while this one waited for the flight lock.
            Some(cached) => Ok(cached.map(|value| (*value).clone())),
            None => match refresh().await {
                Ok(value) => {
                    self.insert(key.clone(), value.clone());

                    Ok(value)
                }
                Err(error) => Err(error),
            },
        }
    }

    /// Inserts an entry for `key`, evicting expired entries and, when still at capacity, the
    /// closest-to-expiring entry first.
    ///
    /// Replaced and evicted values are dropped while the write guard is held. If a `V`
    /// destructor panics, the guard is released while unwinding with the map still structurally
    /// valid; eviction may stop early and the insert that triggered it may not run.
    fn insert(&self, key: K, value: Option<V>) {
        let ttl = if value.is_some() {
            self.ttl
        } else {
            self.negative_ttl
        };
        let entry = Entry {
            value: value.map(Arc::new),
            expires_at: Instant::now() + ttl,
        };

        let mut entries = self.entries.write();

        if entries.len() >= self.capacity && !entries.contains_key(&key) {
            Self::evict(&mut entries, self.capacity);
        }

        entries.insert(key, entry);
    }

    /// Makes room for a new entry by removing expired entries and, when the cache is still at
    /// capacity, the single closest-to-expiring entry.
    ///
    /// Removed entries are dropped while the caller's write guard is held. `retain` and `remove`
    /// are panic-safe, so a panicking `V` destructor leaves the map structurally valid, possibly
    /// with the purge incomplete and the following insert skipped.
    fn evict(entries: &mut HashMap<K, Entry<Option<Arc<V>>>>, capacity: usize) {
        let now = Instant::now();
        let mut expired = 0usize;
        let mut oldest: Option<(&K, Instant)> = None;

        for (key, entry) in entries.iter() {
            if entry.expires_at <= now {
                expired += 1;
            } else if oldest.is_none_or(|(_, oldest_expiry)| entry.expires_at < oldest_expiry) {
                // A strict comparison keeps the first of equally old entries, like `min_by_key`.
                oldest = Some((key, entry.expires_at));
            }
        }

        let oldest = if entries.len() - expired >= capacity {
            oldest.map(|(key, _)| key.clone())
        } else {
            None
        };

        if expired > 0 {
            entries.retain(|_, entry| entry.expires_at > now);
        }

        if let Some(oldest) = oldest {
            entries.remove(&oldest);
        }
    }
}

impl<K, V> fmt::Debug for TtlMap<K, V>
where
    K: fmt::Debug,
    V: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TtlMap")
            .field("entries", &*self.entries.read())
            .field("ttl", &self.ttl)
            .field("negative_ttl", &self.negative_ttl)
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

/// Removes a [`TtlMap`] flight entry when its guard is dropped, including on cancellation.
struct FlightRelease<'a, K: Eq + Hash> {
    /// The flight map the entry lives in.
    flights: &'a Mutex<HashMap<K, Arc<tokio::sync::Mutex<()>>>>,
    /// The key whose entry is released.
    key: &'a K,
}

impl<K: Eq + Hash> Drop for FlightRelease<'_, K> {
    fn drop(&mut self) {
        let mut flights = self.flights.lock();

        // A task that is still queued on the flight keeps its clone and cleans up when it in
        // turn releases, so the entry survives at most until its last holder drops this.
        if flights
            .get(self.key)
            .is_some_and(|flight| flight.try_lock().is_ok())
        {
            flights.remove(self.key);
        }
    }
}

#[cfg(test)]
mod tests;
