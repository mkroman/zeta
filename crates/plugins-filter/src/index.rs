//! In-memory filter index with precompiled wildcard matchers.
//!
//! Filters are indexed by channel and, within each channel, by URL host, so the hot lookup path
//! never scans every filter: a message resolves its channel bucket (and the all-channels
//! bucket), and within it only the filters indexed for the URL's exact host plus the wildcard
//! and host-agnostic entries are considered. All wildcard patterns are compiled once when a
//! filter is inserted, not on every check.

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
    filter: Arc<Filter>,
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
        let filter = Arc::new(filter);
        let host = non_blank(filter.host.as_deref()).map(|host| {
            if is_wildcard(host) {
                HostMatcher::Wild(Pattern::compile(Some(host), Case::Insensitive))
            } else {
                HostMatcher::Exact(host.to_lowercase())
            }
        });

        Self {
            path: Pattern::compile(filter.path.as_deref(), Case::Sensitive),
            nickname: Pattern::compile(filter.nickname.as_deref(), Case::Insensitive),
            username: Pattern::compile(filter.username.as_deref(), Case::Insensitive),
            hostname: Pattern::compile(filter.hostname.as_deref(), Case::Insensitive),
            host,
            filter,
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

    /// Whether the entry matches `host`, `sender` and `url`. The host check is repeated for
    /// wildcard entries only; exact entries are already routed by host.
    fn matches(&self, host: Option<&str>, sender: Option<Sender<'_>>, url: &Url) -> bool {
        if !self.host_matches(host) {
            return false;
        }

        if !self.path.matches(Some(url.path())) {
            return false;
        }

        self.sender_matches(sender)
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

    /// Removes the entries with the given filter ids, dropping host buckets that become empty.
    fn remove_ids(&mut self, ids: &[i32]) {
        self.exact.retain(|_, entries| {
            entries.retain(|entry| !ids.contains(&entry.filter.id));
            !entries.is_empty()
        });

        self.wildcard
            .retain(|entry| !ids.contains(&entry.filter.id));
        self.any_host
            .retain(|entry| !ids.contains(&entry.filter.id));
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
            .map(|entry| entry.filter.as_ref())
    }

    /// Whether any entry in the bucket matches `host`, `sender` and `url`.
    fn matches(&self, host: Option<&str>, sender: Option<Sender<'_>>, url: &Url) -> bool {
        let exact = host
            .and_then(|host| self.exact.get(host))
            .map_or_default(Vec::as_slice);

        exact
            .iter()
            .chain(&self.wildcard)
            .chain(&self.any_host)
            .any(|entry| entry.matches(host, sender, url))
    }

    /// Whether any sender-only filter in the bucket ignores `sender` entirely. Only the
    /// host-agnostic bucket can hold such filters.
    fn ignores(&self, sender: Option<Sender<'_>>) -> bool {
        self.any_host.iter().any(|entry| entry.ignores(sender))
    }
}

/// All filters, indexed by channel.
///
/// The index is keyed by the lowercased channel name; the `None` key holds the filters that
/// apply to all channels. A lookup consults both the channel's bucket and the all-channels
/// bucket, and never touches the filters of other channels.
#[derive(Debug, Default, Clone)]
pub struct FilterIndex {
    channels: HashMap<Option<String>, HostIndex>,
}

impl FilterIndex {
    /// Inserts `filter` into the index.
    pub(super) fn insert(&mut self, filter: Filter) {
        let channel = filter.channel.as_deref().map(str::to_lowercase);
        let entry = Arc::new(Entry::new(filter));

        self.channels.entry(channel).or_default().insert(entry);
    }

    /// Removes the filters with the given ids, dropping channel buckets that become empty.
    pub(super) fn remove_ids(&mut self, ids: &[i32]) {
        self.channels.retain(|_, bucket| {
            bucket.remove_ids(ids);

            !bucket.is_empty()
        });
    }

    /// The number of filters in the index.
    pub(super) fn len(&self) -> usize {
        self.channels.values().flat_map(HostIndex::filters).count()
    }

    /// Whether the index holds no filters at all.
    pub(super) fn is_empty(&self) -> bool {
        self.channels.values().all(HostIndex::is_empty)
    }

    /// All filters in the index, ordered by id.
    pub(super) fn filters(&self) -> Vec<Filter> {
        let mut filters: Vec<Filter> = self
            .channels
            .values()
            .flat_map(HostIndex::filters)
            .cloned()
            .collect();

        filters.sort_unstable_by_key(|filter| filter.id);

        filters
    }

    /// The buckets a lookup in `channel` consults: the channel's own filters and the filters
    /// applying to all channels.
    fn buckets(&self, channel: &str) -> impl Iterator<Item = &HostIndex> {
        let key = channel.to_lowercase();

        [self.channels.get(&Some(key)), self.channels.get(&None)]
            .into_iter()
            .flatten()
    }

    /// Whether any filter applies to `channel` and matches `sender` and `url`.
    pub(super) fn matches(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        if self.is_empty() {
            return false;
        }

        let host = url.host_str().map(str::to_lowercase);

        if self
            .buckets(channel)
            .any(|bucket| bucket.matches(host.as_deref(), sender, url))
        {
            trace!(%channel, url.full = %RedactedUrl::from_url(url), "message matches a filter");

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
            trace!(%channel, "sender matches an ignore filter");

            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::NewFilter;
    use super::*;

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
}
