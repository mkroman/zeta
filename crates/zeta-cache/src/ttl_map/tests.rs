//! Tests for the keyed cache: single-flight refreshes, negative entries, expiry, eviction and
//! the insert/contention benchmarks.

use super::*;

use zeta_test_support::assert_empty;

fn ttl_map(ttl: Duration) -> TtlMap<String, u8> {
    TtlMap::new_with_negative_ttl(ttl, ttl / 2, 4)
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

    assert_empty!(
        cache.flights.lock(),
        "the uncontended flight entry is cleaned up afterwards"
    );
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
    assert_empty!(cache.flights.lock());
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
    assert_empty!(cache.flights.lock());
}

#[tokio::test]
async fn cancelled_refreshes_leave_no_flight_entry() {
    let cache = ttl_map(Duration::from_mins(1));

    let cancelled = tokio::time::timeout(
        Duration::from_millis(10),
        cache.get_or_refresh("a".to_string(), || {
            std::future::pending::<Result<Option<u8>, ()>>()
        }),
    )
    .await;

    assert!(cancelled.is_err(), "the refresh should time out");
    assert_empty!(cache.flights.lock());
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
    let entry_expires_at = |key: &str| cache.entries.read().get(key).map(|entry| entry.expires_at);

    cache
        .get_or_refresh("present".to_string(), || async { Ok::<_, ()>(Some(7)) })
        .await
        .unwrap();

    assert!(entry_expires_at("missing") < entry_expires_at("present"));
}

#[tokio::test]
async fn remove_invalidates_cached_and_negative_entries() {
    let cache = ttl_map(Duration::from_mins(1));
    let mut refreshes = 0;

    cache
        .get_or_refresh("present".to_string(), || async {
            refreshes += 1;
            Ok::<_, ()>(Some(7))
        })
        .await
        .unwrap();
    cache
        .get_or_refresh("missing".to_string(), || async {
            refreshes += 1;
            Ok::<_, ()>(None)
        })
        .await
        .unwrap();
    assert_eq!(refreshes, 2);

    assert!(cache.remove(&"present".to_string()));
    assert!(cache.remove(&"missing".to_string()));
    assert!(!cache.remove(&"missing".to_string()));

    // Both keys refresh again after the invalidation, positive and negative alike.
    cache
        .get_or_refresh("present".to_string(), || async {
            refreshes += 1;
            Ok::<_, ()>(Some(7))
        })
        .await
        .unwrap();
    cache
        .get_or_refresh("missing".to_string(), || async {
            refreshes += 1;
            Ok::<_, ()>(None)
        })
        .await
        .unwrap();

    assert_eq!(refreshes, 4);
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
        .get(&"present".to_string())
        .unwrap()
        .value
        .clone()
        .unwrap();

    assert!(Arc::ptr_eq(&present, &stored));

    cache
        .entries
        .write()
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
    assert_empty!(cache.flights.lock());
}

/// Measures inserts into a full cache, where every insert evicts an entry.
///
/// Ignored by default; run with
/// `cargo test -p zeta-cache --lib -- --ignored --nocapture benchmark_inserts_into_a_full_map`.
#[test]
#[ignore = "benchmark: run explicitly with --ignored --nocapture"]
fn benchmark_inserts_into_a_full_map() {
    use std::time::Instant;

    const CAPACITY: usize = 256;
    const INSERTS: u64 = 10_000;

    let cache: TtlMap<u64, u8> =
        TtlMap::new_with_negative_ttl(Duration::from_hours(1), Duration::from_mins(1), CAPACITY);

    for key in 0..CAPACITY as u64 {
        cache.insert(key, None);
    }

    let start = Instant::now();

    for key in CAPACITY as u64..CAPACITY as u64 + INSERTS {
        cache.insert(std::hint::black_box(key), None);
    }

    let nanos = start.elapsed().as_secs_f64() * 1e9;

    #[allow(clippy::cast_precision_loss)] // iteration counts are far below 2^52
    let per_insert = nanos / INSERTS as f64;

    assert_eq!(cache.entries.read().len(), CAPACITY);
    println!("{CAPACITY}-entry cache: {per_insert:.0} ns/insert");
}

/// Measures writer insert latency while four readers clone 64 KiB values through `peek`.
///
/// Ignored by default; run with
/// `cargo test -p zeta-cache --lib -- --ignored --nocapture benchmark_reader_contention`.
#[test]
#[ignore = "benchmark: run explicitly with --ignored --nocapture"]
fn benchmark_reader_contention() {
    use std::sync::Barrier;
    use std::time::Instant;

    const READERS: usize = 4;
    const READS_PER_READER: usize = 25_000;
    const INSERTS: u64 = 10_000;
    const CAPACITY: usize = 256;
    const VALUE_LEN: usize = 64 * 1024;

    let cache: TtlMap<u64, Vec<u8>> =
        TtlMap::new_with_negative_ttl(Duration::from_hours(1), Duration::from_hours(1), CAPACITY);

    for key in 0..CAPACITY as u64 {
        cache.insert(key, Some(vec![0; VALUE_LEN]));
    }

    // Keep the readers' key in place while the writer churns through new keys.
    cache.entries.write().get_mut(&0).unwrap().expires_at =
        Instant::now() + Duration::from_hours(100);

    let barrier = Barrier::new(READERS + 1);
    let mut start = None;
    let mut latencies = Vec::with_capacity(usize::try_from(INSERTS).unwrap());

    std::thread::scope(|scope| {
        for _ in 0..READERS {
            scope.spawn(|| {
                barrier.wait();

                for _ in 0..READS_PER_READER {
                    std::hint::black_box(cache.peek(&0));
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
    assert_eq!(cache.entries.read().len(), 4);

    // A fifth entry evicts the closest-to-expiring one, keeping the cache bounded.
    cache
        .get_or_refresh("e".to_string(), || async { Ok::<_, ()>(Some(7)) })
        .await
        .unwrap();

    let (len, contains_e) = {
        let entries = cache.entries.read();
        (entries.len(), entries.contains_key(&"e".to_string()))
    };

    assert_eq!(len, 4);
    assert!(contains_e);
}

#[tokio::test]
async fn evicts_one_closest_to_expiring_entry_per_insert_at_capacity() {
    // New entries outlive the seeded ones, so each insert evicts the seed it follows.
    let cache = TtlMap::new_with_negative_ttl(Duration::from_hours(10), Duration::from_hours(1), 4);

    // Give every entry a distinct expiry so the oldest is unambiguous.
    for (key, hours) in [("a", 1), ("b", 2), ("c", 3), ("d", 4)] {
        cache.insert(key.to_string(), Some(7));
        cache
            .entries
            .write()
            .get_mut(&key.to_string())
            .unwrap()
            .expires_at = Instant::now() + Duration::from_hours(hours);
    }

    let keys = || {
        let mut keys: Vec<_> = cache.entries.read().keys().cloned().collect();
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
        .get_mut(&"c".to_string())
        .unwrap()
        .expires_at = Instant::now();
    cache.insert("g".to_string(), Some(7));

    assert_eq!(keys(), ["d", "e", "f", "g"]);
}
