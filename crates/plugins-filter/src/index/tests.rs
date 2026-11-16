//! Tests for the in-memory filter index: host, path, channel, and sender matching,
//! mutation, listing, and the lookup benchmarks.

use url::Url;
use zeta_plugin::Sender;

use super::FilterIndex;
use crate::model::{Filter, NewFilter};

fn filter(id: i32, criteria: NewFilter) -> Filter {
    Filter::from_new_filter(id, criteria)
}

fn new_filter(channel: Option<&str>, host: Option<&str>) -> NewFilter {
    NewFilter {
        channel: channel.map(String::from),
        host: host.map(String::from),
        path: None,
        nickname: None,
        username: None,
        hostname: None,
        created_by: "smoke".into(),
    }
}

fn matches(index: &FilterIndex, channel: &str, url: &str) -> bool {
    index.matches(
        channel,
        Some(Sender::new("nick", "user", "host.example")),
        &url.parse().unwrap(),
    )
}

/// Returns an index pre-populated with the given filters, numbered from 1.
fn index_with(criteria: &[NewFilter]) -> FilterIndex {
    let mut index = FilterIndex::default();

    for (position, criteria) in criteria.iter().enumerate() {
        let id = i32::try_from(position).expect("fewer filters than `i32` allows") + 1;

        index.insert(filter(id, criteria.clone()));
    }

    index
}

#[test]
fn an_empty_index_matches_nothing() {
    let index = FilterIndex::default();

    assert!(index.is_empty());
    assert_eq!(index.len(), 0);
    assert!(!matches(&index, "#chan", "https://imdb.com/title/tt1"));
    assert!(!index.ignores("#chan", Some(Sender::new("mk", "user", "host.example"))));
}

#[test]
fn matches_hosts_by_exact_and_wildcard_patterns() {
    let cases = [
        ("exact host", "imdb.com", "https://imdb.com/title/tt1", true),
        (
            "exact host does not match a subdomain",
            "imdb.com",
            "https://www.imdb.com/title/tt1",
            false,
        ),
        ("unrelated exact host", "imdb.com", "https://dr.dk", false),
        (
            "wildcard host",
            "*.com",
            "https://reuters.com/some-article",
            true,
        ),
        (
            "wildcard host with another tld",
            "*.com",
            "https://dr.dk/some-article",
            false,
        ),
        (
            "uppercase pattern",
            "IMDB.COM",
            "https://imdb.com/title/tt1",
            true,
        ),
        (
            "uppercase url host",
            "imdb.com",
            "https://IMDB.com/title/tt1",
            true,
        ),
    ];

    for (case, pattern, url, expected) in cases {
        let index = index_with(&[new_filter(None, Some(pattern))]);

        assert_eq!(matches(&index, "#chan", url), expected, "for {case}");
    }
}

#[test]
fn suffix_host_patterns_match_subdomains_only() {
    let index = index_with(&[new_filter(None, Some("*.example.com"))]);

    assert!(matches(&index, "#chan", "https://www.example.com/page"));
    assert!(matches(&index, "#chan", "https://a.b.example.com/page"));
    assert!(!matches(&index, "#chan", "https://example.com/page"));
    assert!(!matches(&index, "#chan", "https://evilexample.com/page"));

    // Hostless URLs never match a host pattern, including a suffix one.
    assert!(!index.matches(
        "#chan",
        None,
        &Url::parse("mailto:someone@example.com").unwrap()
    ));

    let uppercase = index_with(&[new_filter(None, Some("*.EXAMPLE.COM"))]);

    assert!(matches(&uppercase, "#chan", "https://WWW.EXAMPLE.COM/page"));
}

#[test]
fn other_wildcard_host_patterns_keep_wildcard_matching() {
    let wild = index_with(&[new_filter(None, Some("*"))]);
    let nested = index_with(&[new_filter(None, Some("*.*.com"))]);
    let middle = index_with(&[new_filter(None, Some("*.com*"))]);

    assert!(wild.matches(
        "#chan",
        None,
        &Url::parse("mailto:someone@example.com").unwrap()
    ));
    assert!(matches(&nested, "#chan", "https://a.b.com/page"));
    assert!(!matches(&nested, "#chan", "https://b.com/page"));
    assert!(matches(&middle, "#chan", "https://reuters.com/page"));
    assert!(matches(&middle, "#chan", "https://reuters.comx"));
}

#[test]
fn scopes_filters_to_channels() {
    let scoped = index_with(&[new_filter(Some("#foo"), Some("dr.dk"))]);

    assert!(matches(&scoped, "#FOO", "https://dr.dk"));
    assert!(!matches(&scoped, "#bar", "https://dr.dk"));

    let all_channels = index_with(&[new_filter(None, Some("dr.dk"))]);

    assert!(matches(&all_channels, "#foo", "https://dr.dk"));
    assert!(matches(&all_channels, "#bar", "https://dr.dk"));
}

#[test]
fn uppercase_channel_lookups_match_case_insensitively() {
    let url: Url = "https://dr.dk".parse().unwrap();
    let sender = Some(Sender::new("nick", "user", "host.example"));

    let index = index_with(&[
        NewFilter {
            nickname: Some("nick".into()),
            ..new_filter(Some("#MixedCase"), None)
        },
        new_filter(Some("#MIXEDCASE"), Some("dr.dk")),
    ]);

    for channel in ["#mixedcase", "#MIXEDCASE", "#MixedCase"] {
        assert!(index.matches(channel, sender, &url), "for {channel}");
        assert!(index.ignores(channel, sender), "for {channel}");
    }

    assert!(!index.matches("#other", sender, &url));
    assert!(!index.ignores("#other", sender));
}

#[test]
fn uppercase_url_hosts_match_without_lowercasing() {
    let exact = index_with(&[new_filter(None, Some("imdb.com"))]);

    assert!(exact.matches(
        "#chan",
        None,
        &"https://IMDB.COM/title/tt1".parse().unwrap()
    ));
    assert!(exact.matches(
        "#chan",
        None,
        &"HTTPS://ImDb.CoM/title/tt1".parse().unwrap()
    ));

    let wildcard = index_with(&[new_filter(None, Some("*.IMDB.com"))]);

    assert!(wildcard.matches(
        "#chan",
        None,
        &"https://WWW.IMDB.COM/title/tt1".parse().unwrap()
    ));
    assert!(!wildcard.matches(
        "#chan",
        None,
        &"https://WWW.EXAMPLE.COM/title/tt1".parse().unwrap()
    ));
}

#[test]
fn sender_criteria_require_a_sender() {
    let index = index_with(&[NewFilter {
        username: Some("*other".into()),
        ..new_filter(None, Some("imdb.com"))
    }]);

    assert!(index.matches(
        "#chan",
        Some(Sender::new("someone", "~cliother", "host.example")),
        &"https://imdb.com/title/tt1".parse().unwrap()
    ));
    assert!(!index.matches(
        "#chan",
        None,
        &"https://imdb.com/title/tt1".parse().unwrap()
    ));
}

#[test]
fn sender_only_filters_ignore_matching_senders() {
    let index = index_with(&[NewFilter {
        nickname: Some("*bot".into()),
        ..new_filter(None, None)
    }]);

    assert!(index.ignores("#chan", Some(Sender::new("a_bot", "user", "host.example"))));
    assert!(!index.ignores(
        "#chan",
        Some(Sender::new("someone", "user", "host.example"))
    ));
    assert!(!index.ignores("#chan", None));

    let user = index_with(&[NewFilter {
        username: Some("*other".into()),
        ..new_filter(None, None)
    }]);

    assert!(user.ignores(
        "#chan",
        Some(Sender::new("someone", "~cliother", "host.example"))
    ));

    let hostname = index_with(&[NewFilter {
        hostname: Some("*.isp.example".into()),
        ..new_filter(None, None)
    }]);

    assert!(hostname.ignores("#chan", Some(Sender::new("mk", "user", "dsl.isp.example"))));
}

#[test]
fn sender_patterns_match_case_insensitively() {
    let index = index_with(&[NewFilter {
        nickname: Some("*BOT".into()),
        ..new_filter(None, None)
    }]);

    assert!(index.ignores("#chan", Some(Sender::new("a_bot", "user", "host.example"))));
}

#[test]
fn filters_with_url_criteria_or_no_criteria_do_not_ignore_senders() {
    let sender = Some(Sender::new("a_bot", "user", "host.example"));

    let host = index_with(&[NewFilter {
        nickname: Some("*bot".into()),
        ..new_filter(None, Some("imdb.com"))
    }]);
    let path = index_with(&[NewFilter {
        nickname: Some("*bot".into()),
        path: Some("/title/*".into()),
        ..new_filter(None, None)
    }]);
    let no_criteria = index_with(&[new_filter(None, None)]);

    assert!(!host.ignores("#chan", sender));
    assert!(!path.ignores("#chan", sender));
    assert!(!no_criteria.ignores("#chan", sender));
}

#[test]
fn sender_ignore_filters_are_scoped_to_channels() {
    let index = index_with(&[NewFilter {
        nickname: Some("mk".into()),
        ..new_filter(Some("#foo"), None)
    }]);

    assert!(index.ignores("#FOO", Some(Sender::new("mk", "user", "host.example"))));
    assert!(!index.ignores("#bar", Some(Sender::new("mk", "user", "host.example"))));
}

#[test]
fn a_filter_without_criteria_ignores_nobody() {
    let index = index_with(&[new_filter(None, None)]);

    assert!(!index.ignores("#chan", Some(Sender::new("mk", "user", "host.example"))));
}

#[test]
fn combines_criteria_with_and() {
    let index = index_with(&[NewFilter {
        nickname: Some("mk".into()),
        ..new_filter(None, Some("dr.dk"))
    }]);

    assert!(index.matches(
        "#chan",
        Some(Sender::new("mk", "user", "host.example")),
        &"https://dr.dk".parse().unwrap()
    ));
    assert!(!index.matches(
        "#chan",
        Some(Sender::new("someoneelse", "user", "host.example")),
        &"https://dr.dk".parse().unwrap()
    ));
    assert!(!index.matches(
        "#chan",
        Some(Sender::new("mk", "user", "host.example")),
        &"https://reuters.com".parse().unwrap()
    ));
}

#[test]
fn paths_are_matched_with_wildcards_and_case_sensitively() {
    let wildcards = index_with(&[NewFilter {
        path: Some("/title/*".into()),
        ..new_filter(None, Some("imdb.com"))
    }]);

    assert!(matches(
        &wildcards,
        "#chan",
        "https://imdb.com/title/tt1375666"
    ));
    assert!(matches(
        &wildcards,
        "#chan",
        "https://imdb.com/title/tt1375666/mediaviewer/rm1"
    ));
    assert!(!matches(
        &wildcards,
        "#chan",
        "https://imdb.com/name/nm0186505"
    ));

    let upper = index_with(&[NewFilter {
        path: Some("/Title/*".into()),
        ..new_filter(None, None)
    }]);

    assert!(!matches(&upper, "#chan", "https://maero.dk/title/x"));
    assert!(matches(&upper, "#chan", "https://maero.dk/Title/x"));
}

#[test]
fn matches_urls_without_a_host_against_hostless_filters() {
    let index = index_with(&[new_filter(None, None)]);

    assert!(index.matches(
        "#chan",
        None,
        &Url::parse("mailto:someone@example.com").unwrap()
    ));

    let wildcard = index_with(&[new_filter(None, Some("*"))]);

    assert!(wildcard.matches(
        "#chan",
        None,
        &Url::parse("mailto:someone@example.com").unwrap()
    ));

    let host = index_with(&[new_filter(None, Some("example.com"))]);

    assert!(!host.matches(
        "#chan",
        None,
        &Url::parse("mailto:someone@example.com").unwrap()
    ));
}

#[test]
fn removes_filters_by_id() {
    let mut index = index_with(&[
        new_filter(None, Some("imdb.com")),
        new_filter(None, Some("dr.dk")),
    ]);

    index.remove_ids(&[1]);

    assert!(!matches(&index, "#chan", "https://imdb.com/title/tt1"));
    assert!(matches(&index, "#chan", "https://dr.dk"));
    assert_eq!(index.len(), 1);

    // Removing the last filter drops the empty channel buckets too.
    index.remove_ids(&[2]);

    assert!(index.is_empty());
    assert_eq!(index.len(), 0);
}

#[test]
fn lists_all_filters() {
    let index = index_with(&[
        new_filter(Some("#foo"), Some("imdb.com")),
        new_filter(None, Some("dr.dk")),
    ]);

    let ids: Vec<_> = index.filters().iter().map(|filter| filter.id).collect();

    assert_eq!(ids, [1, 2]);
    assert_eq!(index.len(), 2);
    assert!(!index.is_empty());
}

/// Returns the fifty-channel, 650-filter index used by the benchmarks.
///
/// The exact-host entries carry a path that does not match the benchmark URL, so a lookup
/// visits all eight entries of the exact bucket before falling through to the wildcard and
/// host-agnostic buckets.
fn benchmark_index() -> FilterIndex {
    const CHANNELS: usize = 50;

    let mut index = FilterIndex::default();
    let mut id = 1;

    for channel in 0..CHANNELS {
        let name = format!("#chan-{channel}");

        for host in 0..8 {
            index.insert(filter(
                id,
                NewFilter {
                    channel: Some(name.clone()),
                    host: Some(format!("site-{channel}-{host}.example")),
                    path: Some("/wanted/*".into()),
                    ..new_filter(None, None)
                },
            ));
            id += 1;
        }

        index.insert(filter(
            id,
            NewFilter {
                channel: Some(name.clone()),
                host: Some("*.wild.example".into()),
                ..new_filter(None, None)
            },
        ));
        id += 1;

        for nickname in 0..4 {
            index.insert(filter(
                id,
                NewFilter {
                    channel: Some(name.clone()),
                    nickname: Some(format!("*bot{nickname}")),
                    ..new_filter(None, None)
                },
            ));
            id += 1;
        }
    }

    index
}

/// Measures the hot lookup paths over a populated fifty-channel index.
///
/// Ignored by default; run with
/// `cargo test -p plugins-filter --lib -- --ignored --nocapture benchmark_matches_and_ignores`.
#[test]
#[ignore = "benchmark: run explicitly with --ignored --nocapture"]
fn benchmark_matches_and_ignores() {
    use std::time::Instant;

    #[allow(clippy::cast_precision_loss)] // iteration counts are far below 2^52
    fn measure(mut operation: impl FnMut() -> bool) -> f64 {
        const ITERATIONS: usize = 100_000;
        const RUNS: usize = 5;

        let mut best = f64::MAX;

        for _ in 0..RUNS {
            let start = Instant::now();

            for _ in 0..ITERATIONS {
                std::hint::black_box(operation());
            }

            let nanos = start.elapsed().as_secs_f64() * 1e9;

            best = best.min(nanos / ITERATIONS as f64);
        }

        best
    }

    let index = benchmark_index();
    let url: Url = "https://site-0-0.example/some/path".parse().unwrap();
    let sender = Sender::new("someone", "user", "host.example");
    // Keep the inputs opaque: with literals the compiler can constant-fold the lowercasing
    // a real lookup does, hiding the allocation the benchmark is meant to measure.
    let channel = std::hint::black_box("#chan-0");
    let url = std::hint::black_box(&url);
    let matches = measure(|| index.matches(channel, Some(sender), url));
    let ignores = measure(|| index.ignores(channel, Some(sender)));

    println!(
        "{} filters: matches {matches:.0} ns/op, ignores {ignores:.0} ns/op",
        index.len()
    );
}

/// Measures the allocations performed while building a populated index.
///
/// Ignored by default; run with
/// `cargo test -p plugins-filter --lib -- --ignored --nocapture benchmark_index_build_bytes`.
#[test]
#[ignore = "measurement: run explicitly with --ignored --nocapture"]
fn benchmark_index_build_bytes() {
    let mut filters = 0;
    let info = allocation_counter::measure(|| {
        let index = benchmark_index();
        filters = index.len();
        std::hint::black_box(index);
    });
    #[allow(clippy::cast_precision_loss)] // the index is far below 2^52 bytes
    let per_filter = info.bytes_total as f64 / filters as f64;

    println!(
        "{filters} filters: count_total {} allocations, bytes_total {} B \
         ({per_filter:.1} B/filter), count_max {} allocations, bytes_max {} B; \
         size_of::<Filter>() = {} B",
        info.count_total,
        info.bytes_total,
        info.count_max,
        info.bytes_max,
        size_of::<Filter>(),
    );
}
