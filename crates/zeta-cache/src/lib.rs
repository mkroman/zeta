//! A single-slot cache whose entries expire after a time-to-live.
//!
//! Shared by plugins that cache an API response (e.g. currency lists or video categories) and
//! refresh it lazily after a configured TTL. Reads do not need to await, matching the pattern
//! of command handlers that peek at the cache synchronously while refreshes happen in `async`
//! code.
//!
//! Also provides [`TtlMap`], a keyed variant whose entries expire
//! individually.

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant};

use arc_swap::ArcSwapOption;

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

/// A cached value with its absolute expiry time.
#[derive(Clone)]
struct Entry<T> {
    value: T,
    expires_at: Instant,
}

impl<T> Entry<T> {
    /// Wraps `value` with an expiry `ttl` from now.
    fn new(value: T, ttl: Duration) -> Self {
        Self {
            value,
            expires_at: Instant::now() + ttl,
        }
    }

    /// Returns whether the entry has not expired yet.
    fn is_fresh(&self) -> bool {
        self.expires_at > Instant::now()
    }
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
    pub fn with_value(value: T, ttl: Duration) -> Self {
        let cache = Self::new(ttl);

        cache.replace(Entry::new(value, ttl));

        cache
    }

    /// Runs `read` with the cached value if one exists, fresh or stale.
    ///
    /// Stale values are served so callers that pair this with a preceding refresh attempt keep
    /// working with the last known data while the source is unavailable.
    pub fn read<R>(&self, read: impl FnOnce(Option<&T>) -> R) -> R {
        let entry = self.entry.load();

        read(entry.as_ref().map(|entry| &entry.value))
    }

    /// Returns whether the cache holds a value that has not expired.
    fn is_fresh(&self) -> bool {
        let entry = self.entry.load();

        entry.as_ref().is_some_and(|entry| entry.is_fresh())
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
        let entry = self.entry.load();

        entry
            .as_ref()
            .filter(|entry| entry.is_fresh())
            .map(|entry| entry.value.clone())
    }

    /// Returns the cached value when it is fresh, refreshing through `refresh` otherwise.
    ///
    /// Unlike [`TtlCache::read`], a stale value is not returned; a missing or expired value
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
    /// no `Clone`; readers access it through [`TtlCache::read`] or [`TtlCache::get`].
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

/// The default number of entries a [`TtlMap`] holds before evicting.
const DEFAULT_TTL_MAP_CAPACITY: usize = 256;

/// The number of concurrent refreshes a [`TtlMap`] preallocates room for.
const DEFAULT_FLIGHT_CAPACITY: usize = 16;

/// A keyed cache whose entries expire individually after a time-to-live.
///
/// Entries cache both values and *negative* results: a refresh that produces no value stores a
/// negative entry so a burst of requests for the same missing resource does not each hit the
/// source. Cached values are returned only while fresh — unlike [`TtlCache`], stale entries are
/// never served.
pub struct TtlMap<K, V> {
    /// The cached entries, keyed by their lookup key.
    ///
    /// A short write guard covers an insert; reads share a read guard.
    entries: RwLock<HashMap<K, Entry<Option<Arc<V>>>>>,
    /// Per-key single-flight locks: concurrent misses for the same key refresh once. Entries
    /// are removed once they are uncontended, so the map does not grow with refreshed keys.
    flights: tokio::sync::Mutex<HashMap<K, Arc<tokio::sync::Mutex<()>>>>,
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
    pub fn with_negative_ttl(ttl: Duration, negative_ttl: Duration, capacity: usize) -> Self {
        Self {
            entries: RwLock::new(HashMap::with_capacity(capacity)),
            flights: tokio::sync::Mutex::new(HashMap::with_capacity(
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
        Self::with_negative_ttl(ttl, ttl / 10, DEFAULT_TTL_MAP_CAPACITY)
    }

    /// Returns the cached value for `key` when it has not expired.
    pub fn get(&self, key: &K) -> Option<V>
    where
        V: Clone,
    {
        self.peek(key).flatten()
    }

    /// Returns the cached entry for `key` when it has not expired, including negative results.
    ///
    /// `Some(None)` is a cached negative result; `None` means nothing is cached for `key`.
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
        let entries = self.entries.read().unwrap_or_else(PoisonError::into_inner);

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
            let mut flights = self.flights.lock().await;

            Arc::clone(flights.entry(key.clone()).or_default())
        };
        let flight_guard = flight.lock().await;

        // No `?` before the release below: every exit — including a failing refresh and the
        // double-check — has to give the flight entry a chance to be cleaned up.
        let outcome = match self.peek_arc(&key) {
            // Another task refreshed the key while this one waited for the flight lock.
            Some(cached) => Ok(cached.map(|value| (*value).clone())),
            None => match refresh().await {
                Ok(value) => {
                    self.insert(key.clone(), value.clone());

                    Ok(value)
                }
                Err(error) => Err(error),
            },
        };

        // Whoever releases the flight last removes the entry, so the flight map does not grow
        // with every refreshed — or every failing — key; a waiter that already holds the clone
        // keeps working, and the double-check above hands it the freshly inserted entry.
        drop(flight_guard);
        self.release_flight(&key).await;

        outcome
    }

    /// Removes `key`'s flight entry once no task holds or waits for it.
    ///
    /// A task that is still queued on the flight keeps its clone and cleans up when it in turn
    /// releases, so the entry survives at most until its last holder runs this.
    async fn release_flight(&self, key: &K)
    where
        K: Send + Sync,
        V: Send + Sync,
    {
        let mut flights = self.flights.lock().await;

        if flights
            .get(key)
            .is_some_and(|flight| flight.try_lock().is_ok())
        {
            flights.remove(key);
        }
    }

    /// Inserts an entry for `key`, evicting expired entries and, when still at capacity, the
    /// closest-to-expiring entry first.
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

        let mut entries = self.entries.write().unwrap_or_else(PoisonError::into_inner);

        if entries.len() >= self.capacity && !entries.contains_key(&key) {
            Self::evict(&mut entries, self.capacity);
        }

        entries.insert(key, entry);
    }

    /// Makes room for a new entry by removing expired entries and, when the cache is still at
    /// capacity, the single closest-to-expiring entry.
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

#[cfg(test)]
mod ttl_map_tests {
    use super::*;

    use zeta_test_support::assert_empty;

    fn ttl_map(ttl: Duration) -> TtlMap<String, u8> {
        TtlMap::with_negative_ttl(ttl, ttl / 2, 4)
    }

    #[tokio::test]
    async fn get_or_refresh_populates_and_reuses_entries() {
        let cache = ttl_map(Duration::from_mins(1));
        let mut refreshes = 0;

        let first = cache
            .get_or_refresh("a".to_string(), || async {
                refreshes += 1;
                Ok::<_, ()>(Some(7))
            })
            .await
            .unwrap();
        let second = cache
            .get_or_refresh("a".to_string(), || async {
                refreshes += 1;
                Ok::<_, ()>(Some(9))
            })
            .await
            .unwrap();

        assert_eq!(first, Some(7));
        assert_eq!(second, Some(7));
        assert_eq!(refreshes, 1);
        assert_eq!(cache.get(&"a".to_string()), Some(7));

        // The uncontended flight entry is cleaned up afterwards.
        assert_empty!(cache.flights.lock().await);
    }

    #[tokio::test]
    async fn concurrent_misses_for_the_same_key_refresh_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let cache = Arc::new(ttl_map(Duration::from_mins(1)));
        let refreshes = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();

        for _ in 0..8 {
            let cache = Arc::clone(&cache);
            let refreshes = Arc::clone(&refreshes);

            handles.push(tokio::spawn(async move {
                cache
                    .get_or_refresh("a".to_string(), || async {
                        refreshes.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;

                        Ok::<_, ()>(Some(7))
                    })
                    .await
                    .unwrap()
            }));
        }

        for handle in handles {
            assert_eq!(handle.await.unwrap(), Some(7));
        }

        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(cache.get(&"a".to_string()), Some(7));

        // The last flight holder cleaned up, even though every waiter went through the
        // double-check instead of refreshing.
        assert_empty!(cache.flights.lock().await);
    }

    #[tokio::test]
    async fn concurrent_failures_leave_no_flight_entry() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let cache = Arc::new(ttl_map(Duration::from_mins(1)));
        let attempts = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();

        for _ in 0..8 {
            let cache = Arc::clone(&cache);
            let attempts = Arc::clone(&attempts);

            handles.push(tokio::spawn(async move {
                cache
                    .get_or_refresh("a".to_string(), || async {
                        attempts.fetch_add(1, Ordering::SeqCst);

                        Err::<Option<u8>, _>("denied")
                    })
                    .await
            }));
        }

        for handle in handles {
            assert_eq!(handle.await.unwrap(), Err("denied"));
        }

        // Nothing is ever cached, so every task retried the refresh after the previous one
        // released its flight — and every one of them released its own afterwards.
        assert_eq!(attempts.load(Ordering::SeqCst), 8);
        assert_eq!(cache.get(&"a".to_string()), None);
        assert_empty!(cache.flights.lock().await);
    }

    #[tokio::test]
    async fn keys_expire_independently() {
        let cache = ttl_map(Duration::from_mins(1));

        cache
            .get_or_refresh("a".to_string(), || async { Ok::<_, ()>(Some(7)) })
            .await
            .unwrap();
        cache
            .get_or_refresh("b".to_string(), || async { Ok::<_, ()>(Some(9)) })
            .await
            .unwrap();

        // Expire only `a`'s entry.
        cache
            .entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&"a".to_string())
            .unwrap()
            .expires_at = Instant::now();

        assert_eq!(cache.get(&"a".to_string()), None);
        assert_eq!(cache.get(&"b".to_string()), Some(9));

        let refreshes = std::sync::atomic::AtomicU8::new(0);
        let value = cache
            .get_or_refresh("a".to_string(), || async {
                refreshes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok::<_, ()>(Some(11))
            })
            .await
            .unwrap();

        assert_eq!(value, Some(11));
        assert_eq!(refreshes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn negative_results_are_cached_and_expire_sooner() {
        let cache = ttl_map(Duration::from_mins(1));
        let mut refreshes = 0;

        let first = cache
            .get_or_refresh("missing".to_string(), || async {
                refreshes += 1;
                Ok::<_, ()>(None)
            })
            .await
            .unwrap();
        let second = cache
            .get_or_refresh("missing".to_string(), || async {
                refreshes += 1;
                Ok::<_, ()>(None)
            })
            .await
            .unwrap();

        assert_eq!(first, None);
        assert_eq!(second, None);
        assert_eq!(refreshes, 1);

        // Negative results expire before values do.
        let entry_expires_at = |key: &str| {
            cache
                .entries
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .get(key)
                .map(|entry| entry.expires_at)
        };

        cache
            .get_or_refresh("present".to_string(), || async { Ok::<_, ()>(Some(7)) })
            .await
            .unwrap();

        assert!(entry_expires_at("missing") < entry_expires_at("present"));
    }

    #[tokio::test]
    async fn peek_arc_preserves_positive_and_negative_entries() {
        let cache = ttl_map(Duration::from_mins(1));

        cache
            .get_or_refresh("present".to_string(), || async { Ok::<_, ()>(Some(7)) })
            .await
            .unwrap();
        cache
            .get_or_refresh("missing".to_string(), || async { Ok::<_, ()>(None) })
            .await
            .unwrap();

        let present = cache
            .peek_arc(&"present".to_string())
            .expect("positive entry is cached")
            .expect("positive entry has a value");

        assert_eq!(*present, 7);
        assert!(
            cache
                .peek_arc(&"missing".to_string())
                .is_some_and(|entry| entry.is_none())
        );
        assert!(cache.peek_arc(&"absent".to_string()).is_none());

        // The returned value shares the cached allocation instead of copying it.
        let stored = cache
            .entries
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&"present".to_string())
            .unwrap()
            .value
            .clone()
            .unwrap();

        assert!(Arc::ptr_eq(&present, &stored));

        cache
            .entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&"present".to_string())
            .unwrap()
            .expires_at = Instant::now();

        assert!(cache.peek_arc(&"present".to_string()).is_none());
    }

    #[tokio::test]
    async fn errors_are_not_cached() {
        let cache = ttl_map(Duration::from_mins(1));
        let mut refreshes = 0;

        let error = cache
            .get_or_refresh("a".to_string(), || async {
                refreshes += 1;
                Err::<Option<u8>, _>("denied")
            })
            .await
            .unwrap_err();

        assert_eq!(error, "denied");
        assert_eq!(refreshes, 1);
        assert_eq!(cache.get(&"a".to_string()), None);

        // A failing refresh must not leave its flight entry behind: unlike `entries`, the
        // flight map is neither bounded nor expired.
        assert_empty!(cache.flights.lock().await);
    }

    /// Measures inserts into a full cache, where every insert evicts an entry.
    ///
    /// Ignored by default; run with
    /// `cargo test -p zeta-cache --lib -- --ignored --nocapture benchmark_inserts_into_a_full_map`.
    #[test]
    #[ignore = "benchmark: run explicitly with --ignored --nocapture"]
    fn benchmark_inserts_into_a_full_map() {
        use std::hint::black_box;
        use std::time::Instant;

        const CAPACITY: usize = 256;
        const INSERTS: u64 = 10_000;

        let cache: TtlMap<u64, u8> =
            TtlMap::with_negative_ttl(Duration::from_hours(1), Duration::from_mins(1), CAPACITY);

        for key in 0..CAPACITY as u64 {
            cache.insert(key, None);
        }

        let start = Instant::now();

        for key in CAPACITY as u64..CAPACITY as u64 + INSERTS {
            cache.insert(black_box(key), None);
        }

        let nanos = start.elapsed().as_secs_f64() * 1e9;

        #[allow(clippy::cast_precision_loss)] // iteration counts are far below 2^52
        let per_insert = nanos / INSERTS as f64;

        assert_eq!(
            cache
                .entries
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
            CAPACITY
        );
        println!("{CAPACITY}-entry cache: {per_insert:.0} ns/insert");
    }

    /// Measures writer insert latency while four readers clone 64 KiB values through `peek`.
    ///
    /// Ignored by default; run with
    /// `cargo test -p zeta-cache --lib -- --ignored --nocapture benchmark_reader_contention`.
    #[test]
    #[ignore = "benchmark: run explicitly with --ignored --nocapture"]
    fn benchmark_reader_contention() {
        use std::hint::black_box;
        use std::sync::Barrier;
        use std::time::Instant;

        const READERS: usize = 4;
        const READS_PER_READER: usize = 25_000;
        const INSERTS: u64 = 10_000;
        const CAPACITY: usize = 256;
        const VALUE_LEN: usize = 64 * 1024;

        let cache: TtlMap<u64, Vec<u8>> =
            TtlMap::with_negative_ttl(Duration::from_hours(1), Duration::from_hours(1), CAPACITY);

        for key in 0..CAPACITY as u64 {
            cache.insert(key, Some(vec![0; VALUE_LEN]));
        }

        // Keep the readers' key in place while the writer churns through new keys.
        cache
            .entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&0)
            .unwrap()
            .expires_at = Instant::now() + Duration::from_hours(100);

        let barrier = Barrier::new(READERS + 1);
        let mut start = None;
        let mut latencies = Vec::with_capacity(usize::try_from(INSERTS).unwrap());

        std::thread::scope(|scope| {
            for _ in 0..READERS {
                scope.spawn(|| {
                    barrier.wait();

                    for _ in 0..READS_PER_READER {
                        black_box(cache.peek(&0));
                    }
                });
            }

            barrier.wait();
            start = Some(Instant::now());

            for key in CAPACITY as u64..CAPACITY as u64 + INSERTS {
                let value = vec![0; VALUE_LEN];
                let insert_start = Instant::now();
                cache.insert(key, Some(value));
                latencies.push(insert_start.elapsed());
            }
        });

        let elapsed = start.expect("writer started").elapsed();
        latencies.sort_unstable();
        let median = latencies[latencies.len() / 2];
        let p95 = latencies[latencies.len() * 95 / 100];
        let max = latencies.last().copied().unwrap_or_default();

        println!(
            "{READERS} readers x {READS_PER_READER} reads, {INSERTS} inserts: \
             median insert {:.0} ns, p95 {:.0} ns, max {:.0} ns, total {:.1} ms",
            median.as_secs_f64() * 1e9,
            p95.as_secs_f64() * 1e9,
            max.as_secs_f64() * 1e9,
            elapsed.as_secs_f64() * 1e3,
        );
    }

    #[tokio::test]
    async fn evicts_when_full() {
        let cache = ttl_map(Duration::from_hours(1));

        for key in ["a", "b", "c", "d"] {
            cache
                .get_or_refresh(key.to_string(), || async { Ok::<_, ()>(Some(7)) })
                .await
                .unwrap();
        }
        assert_eq!(
            cache
                .entries
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
            4
        );

        // A fifth entry evicts the closest-to-expiring one, keeping the cache bounded.
        cache
            .get_or_refresh("e".to_string(), || async { Ok::<_, ()>(Some(7)) })
            .await
            .unwrap();

        let (len, contains_e) = {
            let entries = cache.entries.read().unwrap_or_else(PoisonError::into_inner);
            (entries.len(), entries.contains_key(&"e".to_string()))
        };

        assert_eq!(len, 4);
        assert!(contains_e);
    }

    #[tokio::test]
    async fn evicts_one_closest_to_expiring_entry_per_insert_at_capacity() {
        // New entries outlive the seeded ones, so each insert evicts the seed it follows.
        let cache = TtlMap::with_negative_ttl(Duration::from_hours(10), Duration::from_hours(1), 4);

        // Give every entry a distinct expiry so the oldest is unambiguous.
        for (key, hours) in [("a", 1), ("b", 2), ("c", 3), ("d", 4)] {
            cache.insert(key.to_string(), Some(7));
            cache
                .entries
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .get_mut(&key.to_string())
                .unwrap()
                .expires_at = Instant::now() + Duration::from_hours(hours);
        }

        let keys = || {
            let mut keys: Vec<_> = cache
                .entries
                .read()
                .unwrap_or_else(PoisonError::into_inner)
                .keys()
                .cloned()
                .collect();
            keys.sort_unstable();

            keys
        };

        // One insert evicts only the closest-to-expiring entry.
        cache.insert("e".to_string(), Some(7));

        assert_eq!(keys(), ["b", "c", "d", "e"]);

        // The next insert evicts the next-closest entry.
        cache.insert("f".to_string(), Some(7));

        assert_eq!(keys(), ["c", "d", "e", "f"]);

        // An expired entry is retained away without evicting a fresh one.
        cache
            .entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(&"c".to_string())
            .unwrap()
            .expires_at = Instant::now();
        cache.insert("g".to_string(), Some(7));

        assert_eq!(keys(), ["d", "e", "f", "g"]);
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
    async fn concurrent_misses_refresh_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let cache = Arc::new(cache_with(Duration::from_mins(1)));
        let refreshes = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();

        for _ in 0..8 {
            let cache = Arc::clone(&cache);
            let refreshes = Arc::clone(&refreshes);

            handles.push(tokio::spawn(async move {
                cache
                    .get_or_refresh(|| async {
                        refreshes.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;

                        Ok::<_, ()>(7)
                    })
                    .await
                    .unwrap()
            }));
        }

        for handle in handles {
            assert_eq!(handle.await.unwrap(), 7);
        }

        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn get_or_refresh_with_honors_the_refresh_provided_ttl() {
        // The cache-wide TTL is zero: only the refresh-provided TTL can keep the value fresh.
        let cache = TtlCache::new(Duration::ZERO);
        let mut refreshes = 0;

        let first = cache
            .get_or_refresh_with(|| async {
                refreshes += 1;
                Ok::<_, ()>((7, Duration::from_mins(1)))
            })
            .await
            .unwrap();
        let second = cache
            .get_or_refresh_with(|| async {
                refreshes += 1;
                Ok::<_, ()>((9, Duration::from_mins(1)))
            })
            .await
            .unwrap();

        assert_eq!(first, 7);
        assert_eq!(second, 7);
        assert_eq!(refreshes, 1);
        assert_eq!(cache.get(), Some(7));

        // A refresh-provided zero TTL overrides a long cache-wide TTL: the value is immediately
        // stale, so the next read refreshes again.
        let cache = TtlCache::new(Duration::from_hours(1));
        let mut refreshes = 0;

        cache
            .get_or_refresh_with(|| async {
                refreshes += 1;
                Ok::<_, ()>((7, Duration::ZERO))
            })
            .await
            .unwrap();
        let value = cache
            .get_or_refresh_with(|| async {
                refreshes += 1;
                Ok::<_, ()>((9, Duration::from_mins(1)))
            })
            .await
            .unwrap();

        assert_eq!(value, 9);
        assert_eq!(refreshes, 2);
        assert_eq!(cache.get(), Some(9));
    }

    #[tokio::test]
    async fn get_or_refresh_with_refreshes_once_for_concurrent_misses() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let cache = Arc::new(cache_with(Duration::from_mins(1)));
        let refreshes = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();

        for _ in 0..8 {
            let cache = Arc::clone(&cache);
            let refreshes = Arc::clone(&refreshes);

            handles.push(tokio::spawn(async move {
                cache
                    .get_or_refresh_with(|| async {
                        refreshes.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;

                        Ok::<_, ()>((7, Duration::from_mins(5)))
                    })
                    .await
                    .unwrap()
            }));
        }

        for handle in handles {
            assert_eq!(handle.await.unwrap(), 7);
        }

        assert_eq!(refreshes.load(Ordering::SeqCst), 1);
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
        cache.entry.store(Some(Arc::new(Entry {
            value: 7,
            expires_at: Instant::now(),
        })));

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
        cache
            .get_or_refresh(|| async { Ok::<_, ()>(7) })
            .await
            .unwrap();

        // Expire the entry directly, so the test does not depend on wall-clock timing.
        let mut entry = cache.entry.load_full().expect("cache is populated");
        Arc::make_mut(&mut entry).expires_at = Instant::now();
        cache.entry.store(Some(entry));

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

        cache
            .get_or_refresh(|| async { Ok::<_, ()>(7) })
            .await
            .unwrap();
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
