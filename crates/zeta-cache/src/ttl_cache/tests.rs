//! Tests for the single-slot cache: refresh reuse, single-flight refreshes, refreshes with a
//! provided TTL, expiry and stale reads.

use super::*;

use std::time::Instant;

#[allow(clippy::redundant_closure_for_method_calls)] // a bare `Option::copied` is ambiguous
fn cached_value(cache: &TtlCache<u8>) -> Option<u8> {
    cache.with_value(|value| value.copied())
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
    // ...but `with_value` still serves it, and a refresh replaces it (though a zero TTL makes
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
