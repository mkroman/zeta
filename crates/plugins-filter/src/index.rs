//! In-memory filter index with precompiled wildcard matchers.
//!
//! Filters are indexed by channel and, within each channel, by URL host, so the hot lookup path
//! never scans every filter: a message resolves its channel bucket (and the all-channels
//! bucket), and within it only the filters indexed for the URL's exact host plus the wildcard
//! and host-agnostic entries are considered. All wildcard patterns are compiled once when a
//! filter is inserted, not on every check.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use tracing::trace;
use url::Url;

use super::model::{Case, Filter, Pattern};
use zeta_plugin::Sender;
use zeta_url::RedactedUrl;

/// Whether `pattern` contains wildcards.
pub fn is_wildcard(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

/// Returns `pattern` trimmed, or `None` when it is missing or blank.
fn non_blank(pattern: Option<&str>) -> Option<&str> {
    pattern.map(str::trim).filter(|pattern| !pattern.is_empty())
}

/// The compiled host pattern of a filter.
#[derive(Debug, Clone)]
enum HostMatcher {
    /// An exact host, matched for equality.
    Exact(String),
    /// A wildcard host pattern.
    Wild(Pattern),
}

/// A filter with its wildcard patterns precompiled for matching.
///
/// Shared behind an [`Arc`] so cloning the index on add/delete does not copy every filter and
/// matcher.
#[derive(Debug)]
struct Entry {
    /// The raw filter.
    filter: Filter,
    /// The compiled host pattern, if any.
    host: Option<HostMatcher>,
    /// The compiled URL path pattern.
    path: Pattern,
    /// The compiled sender nickname pattern.
    nickname: Pattern,
    /// The compiled sender username pattern.
    username: Pattern,
    /// The compiled sender hostname pattern.
    hostname: Pattern,
}

impl Entry {
    /// Compiles `filter` into an entry.
    fn new(filter: Filter) -> Self {
        let host = non_blank(filter.host.as_deref()).map(|host| {
            if is_wildcard(host) {
                HostMatcher::Wild(Pattern::compile(Some(host), Case::Insensitive))
            } else {
                HostMatcher::Exact(host.to_lowercase())
            }
        });
        let path = Pattern::compile(filter.path.as_deref(), Case::Sensitive);
        let nickname = Pattern::compile(filter.nickname.as_deref(), Case::Insensitive);
        let username = Pattern::compile(filter.username.as_deref(), Case::Insensitive);
        let hostname = Pattern::compile(filter.hostname.as_deref(), Case::Insensitive);

        Self {
            filter,
            host,
            path,
            nickname,
            username,
            hostname,
        }
    }

    /// Whether the host pattern of the entry matches `host` (already lowercased). Hostless URLs
    /// match wildcard patterns as if their host were empty, so `*` matches every URL.
    fn host_matches(&self, host: Option<&str>) -> bool {
        match &self.host {
            None => true,
            Some(HostMatcher::Exact(expected)) => host == Some(expected.as_str()),
            Some(HostMatcher::Wild(pattern)) => pattern.matches(Some(host.unwrap_or(""))),
        }
    }

    /// Whether the entry's sender criteria match `sender`, or are all unset.
    fn sender_matches(&self, sender: Option<Sender<'_>>) -> bool {
        let Some(sender) = sender else {
            // A set sender criterion needs a sender to match against.
            return self.nickname.is_unset()
                && self.username.is_unset()
                && self.hostname.is_unset();
        };

        self.nickname.matches(Some(sender.nick))
            && self.username.matches(Some(sender.username))
            && self.hostname.matches(Some(sender.hostname))
    }

    /// Whether the entry matches `host`, `sender` and `url`.
    ///
    /// Only wildcard entries need this: exact entries are routed by their host and hostless
    /// entries pass the host check unconditionally, so both use [`Entry::matches_after_host`].
    fn matches(&self, host: Option<&str>, sender: Option<Sender<'_>>, url: &Url) -> bool {
        self.host_matches(host) && self.matches_after_host(sender, url)
    }

    /// Whether the entry matches `sender` and `url` once its host is already known to match.
    fn matches_after_host(&self, sender: Option<Sender<'_>>, url: &Url) -> bool {
        self.path.matches(Some(url.path())) && self.sender_matches(sender)
    }

    /// Whether the entry ignores `sender` entirely: it has no URL criteria (host and path
    /// unset) and at least one sender criterion, which matches. The sender-criteria
    /// requirement keeps a criteria-less filter — which the command cannot create — from
    /// ignoring everyone.
    fn ignores(&self, sender: Option<Sender<'_>>) -> bool {
        if self.host.is_some() || !self.path.is_unset() {
            return false;
        }

        if self.nickname.is_unset() && self.username.is_unset() && self.hostname.is_unset() {
            return false;
        }

        self.sender_matches(sender)
    }
}

/// The filters of a single channel (or of all channels), indexed by URL host.
#[derive(Debug, Default, Clone)]
struct HostIndex {
    /// Filters with an exact host pattern, keyed by the lowercased host.
    exact: HashMap<String, Vec<Arc<Entry>>>,
    /// Filters with a wildcard host pattern.
    wildcard: Vec<Arc<Entry>>,
    /// Filters without a host pattern.
    any_host: Vec<Arc<Entry>>,
}

impl HostIndex {
    /// Inserts `entry` into the bucket its host pattern routes to.
    fn insert(&mut self, entry: Arc<Entry>) {
        match &entry.host {
            Some(HostMatcher::Exact(host)) => {
                self.exact.entry(host.clone()).or_default().push(entry);
            }
            Some(HostMatcher::Wild(_)) => self.wildcard.push(entry),
            None => self.any_host.push(entry),
        }
    }

    /// Removes the entries with the given filter ids, dropping host buckets that become empty,
    /// and returns how many entries were removed.
    fn remove_ids(&mut self, ids: &[i32]) -> usize {
        let mut removed = 0;

        self.exact.retain(|_, entries| {
            let before = entries.len();
            entries.retain(|entry| !ids.contains(&entry.filter.id));
            removed += before - entries.len();

            !entries.is_empty()
        });

        let before = self.wildcard.len();
        self.wildcard
            .retain(|entry| !ids.contains(&entry.filter.id));
        removed += before - self.wildcard.len();

        let before = self.any_host.len();
        self.any_host
            .retain(|entry| !ids.contains(&entry.filter.id));
        removed += before - self.any_host.len();

        removed
    }

    /// Whether the bucket holds no filters at all.
    fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.wildcard.is_empty() && self.any_host.is_empty()
    }

    /// All filters in the bucket, in unspecified order.
    fn filters(&self) -> impl Iterator<Item = &Filter> {
        self.exact
            .values()
            .flatten()
            .chain(&self.wildcard)
            .chain(&self.any_host)
            .map(|entry| &entry.filter)
    }

    /// Whether any entry in the bucket matches `host`, `sender` and `url`.
    fn matches(&self, host: Option<&str>, sender: Option<Sender<'_>>, url: &Url) -> bool {
        let exact = host
            .and_then(|host| self.exact.get(host))
            .map_or_default(Vec::as_slice);

        exact
            .iter()
            .any(|entry| entry.matches_after_host(sender, url))
            || self
                .wildcard
                .iter()
                .any(|entry| entry.matches(host, sender, url))
            || self
                .any_host
                .iter()
                .any(|entry| entry.matches_after_host(sender, url))
    }

    /// Whether any sender-only filter in the bucket ignores `sender` entirely. Only the
    /// host-agnostic bucket can hold such filters.
    fn ignores(&self, sender: Option<Sender<'_>>) -> bool {
        self.any_host.iter().any(|entry| entry.ignores(sender))
    }
}

/// Returns the index key of `channel`: its lowercased name, borrowed when the channel is plain
/// ASCII without uppercase letters, which is the common case.
fn channel_key(channel: &str) -> Cow<'_, str> {
    if channel.bytes().any(|byte| byte.is_ascii_uppercase()) || !channel.is_ascii() {
        Cow::Owned(channel.to_lowercase())
    } else {
        Cow::Borrowed(channel)
    }
}

/// All filters, indexed by channel.
///
/// The index is keyed by the lowercased channel name; [`all_channels`](Self::all_channels)
/// holds the filters that apply to all channels. A lookup consults both, and never touches the
/// filters of other channels.
#[derive(Debug, Default, Clone)]
pub struct FilterIndex {
    /// The channel buckets, keyed by lowercased channel name.
    channels: HashMap<String, HostIndex>,
    /// The filters that apply to all channels.
    all_channels: HostIndex,
    /// The number of filters in the index, kept exact by [`FilterIndex::insert`] and
    /// [`FilterIndex::remove_ids`] so [`FilterIndex::len`] and [`FilterIndex::is_empty`] are
    /// constant-time.
    count: usize,
}

impl FilterIndex {
    /// Inserts `filter` into the index.
    pub(super) fn insert(&mut self, filter: Filter) {
        let channel = filter.channel.as_deref().map(str::to_lowercase);
        let entry = Arc::new(Entry::new(filter));

        match channel {
            Some(channel) => self.channels.entry(channel).or_default().insert(entry),
            None => self.all_channels.insert(entry),
        }

        self.count += 1;
    }

    /// Removes the filters with the given ids, dropping channel buckets that become empty.
    pub(super) fn remove_ids(&mut self, ids: &[i32]) {
        let mut removed = self.all_channels.remove_ids(ids);

        self.channels.retain(|_, bucket| {
            removed += bucket.remove_ids(ids);

            !bucket.is_empty()
        });

        self.count -= removed;
    }

    /// The number of filters in the index.
    pub(super) const fn len(&self) -> usize {
        self.count
    }

    /// Whether the index holds no filters at all.
    pub(super) const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// All filters in the index, ordered by id.
    pub(super) fn filters(&self) -> Vec<Filter> {
        let mut filters: Vec<Filter> = self
            .channels
            .values()
            .chain(std::iter::once(&self.all_channels))
            .flat_map(HostIndex::filters)
            .cloned()
            .collect();

        filters.sort_unstable_by_key(|filter| filter.id);

        filters
    }

    /// The buckets a lookup in `channel` consults: the channel's own filters and the filters
    /// applying to all channels.
    fn buckets(&self, channel: &str) -> impl Iterator<Item = &HostIndex> {
        let key = channel_key(channel);
        let all_channels = (!self.all_channels.is_empty()).then_some(&self.all_channels);

        [self.channels.get(key.as_ref()), all_channels]
            .into_iter()
            .flatten()
    }

    /// Whether any filter applies to `channel` and matches `sender` and `url`.
    pub(super) fn matches(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        if self.is_empty() {
            return false;
        }

        // `Url::host_str` is already lowercased for http(s) URLs, so it is used as-is.
        let host = url.host_str();

        if self
            .buckets(channel)
            .any(|bucket| bucket.matches(host, sender, url))
        {
            trace!(url.full = %RedactedUrl::from_url(url), "message matches a filter");

            true
        } else {
            false
        }
    }

    /// Whether any filter applies to `channel` that ignores `sender` entirely.
    pub(super) fn ignores(&self, channel: &str, sender: Option<Sender<'_>>) -> bool {
        if self.is_empty() {
            return false;
        }

        if self.buckets(channel).any(|bucket| bucket.ignores(sender)) {
            trace!("sender matches an ignore filter");

            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use stats_alloc::{INSTRUMENTED_SYSTEM, Region, StatsAlloc};
    use std::alloc::System;

    use super::super::model::NewFilter;
    use super::*;

    #[global_allocator]
    static GLOBAL: &StatsAlloc<System> = &INSTRUMENTED_SYSTEM;

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
        use std::hint::black_box;
        use std::time::Instant;

        #[allow(clippy::cast_precision_loss)] // iteration counts are far below 2^52
        fn measure(mut operation: impl FnMut() -> bool) -> f64 {
            const ITERATIONS: usize = 100_000;
            const RUNS: usize = 5;

            let mut best = f64::MAX;

            for _ in 0..RUNS {
                let start = Instant::now();

                for _ in 0..ITERATIONS {
                    black_box(operation());
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
        let channel = black_box("#chan-0");
        let url = black_box(&url);
        let matches = measure(|| index.matches(channel, Some(sender), url));
        let ignores = measure(|| index.ignores(channel, Some(sender)));

        println!(
            "{} filters: matches {matches:.0} ns/op, ignores {ignores:.0} ns/op",
            index.len()
        );
    }

    /// Measures the heap bytes retained by a populated index.
    ///
    /// Ignored by default; run with
    /// `cargo test -p plugins-filter --lib -- --ignored --nocapture benchmark_index_build_bytes`.
    #[test]
    #[ignore = "measurement: run explicitly with --ignored --nocapture"]
    fn benchmark_index_build_bytes() {
        use std::hint::black_box;

        let region = Region::new(GLOBAL);
        let index = benchmark_index();
        let stats = region.change();
        let allocated = isize::try_from(stats.bytes_allocated).unwrap();
        let deallocated = isize::try_from(stats.bytes_deallocated).unwrap();
        let live = allocated - deallocated + stats.bytes_reallocated;
        #[allow(clippy::cast_precision_loss)] // the index is far below 2^52 bytes
        let per_filter = live as f64 / index.len() as f64;

        println!(
            "{} filters: allocated {} B, deallocated {} B, reallocated {} B, live {} B \
             ({per_filter:.1} B/filter), {} allocations; size_of::<Filter>() = {} B",
            index.len(),
            stats.bytes_allocated,
            stats.bytes_deallocated,
            stats.bytes_reallocated,
            live,
            stats.allocations,
            size_of::<Filter>(),
        );

        black_box(index);
    }
}
