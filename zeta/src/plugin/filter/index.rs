//! In-memory filter index with precompiled wildcard matchers.
//!
//! Filters are indexed by channel and, within each channel, by URL host, so the hot lookup path
//! never scans every filter: a message resolves its channel bucket (and the all-channels
//! bucket), and within it only the filters indexed for the URL's exact host plus the wildcard
//! and host-agnostic entries are considered. All wildcard patterns are compiled once when a
//! filter is inserted, not on every check.

use std::collections::HashMap;

use tracing::trace;
use url::Url;
use wildmatch::WildMatch;

use super::model::Filter;
use zeta_plugin::Sender;

/// Whether `pattern` contains wildcards.
pub(super) fn is_wildcard(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('?')
}

/// Compiles a case-insensitive wildcard pattern, returning `None` for empty patterns.
fn compile_pattern(pattern: Option<&str>) -> Option<WildMatch> {
    pattern
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .map(|pattern| WildMatch::new(&pattern.to_lowercase()))
}

/// Compiles a case-sensitive wildcard pattern for URL paths, returning `None` for empty
/// patterns.
fn compile_path(pattern: Option<&str>) -> Option<WildMatch> {
    pattern
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .map(WildMatch::new)
}

/// The compiled host pattern of a filter.
#[derive(Debug)]
enum HostMatcher {
    /// An exact host, matched for equality.
    Exact(String),
    /// A wildcard host pattern.
    Wild(WildMatch),
}

/// A filter with its wildcard patterns precompiled for matching.
#[derive(Debug)]
struct Entry {
    /// The raw filter.
    filter: Filter,
    /// The compiled host pattern, if any.
    host: Option<HostMatcher>,
    /// The compiled URL path pattern, if any.
    path: Option<WildMatch>,
    /// The compiled sender nickname pattern, if any.
    nickname: Option<WildMatch>,
    /// The compiled sender username pattern, if any.
    username: Option<WildMatch>,
    /// The compiled sender hostname pattern, if any.
    hostname: Option<WildMatch>,
}

impl Entry {
    /// Compiles `filter` into an entry.
    fn new(filter: Filter) -> Entry {
        let host = filter.host.as_deref().map(|host| {
            let host = host.trim();

            if is_wildcard(host) {
                HostMatcher::Wild(WildMatch::new(&host.to_lowercase()))
            } else {
                HostMatcher::Exact(host.to_lowercase())
            }
        });

        Entry {
            path: compile_path(filter.path.as_deref()),
            nickname: compile_pattern(filter.nickname.as_deref()),
            username: compile_pattern(filter.username.as_deref()),
            hostname: compile_pattern(filter.hostname.as_deref()),
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
            Some(HostMatcher::Wild(pattern)) => pattern.matches(host.unwrap_or("")),
        }
    }

    /// Whether a case-insensitive pattern matches the sender field, or is unset.
    fn field_matches(pattern: Option<&WildMatch>, value: Option<&str>) -> bool {
        match (pattern, value) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(pattern), Some(value)) => pattern.matches(&value.to_lowercase()),
        }
    }

    /// Whether the entry matches `host`, `sender` and `url`. The host check is repeated for
    /// wildcard entries only; exact entries are already routed by host.
    fn matches(&self, host: Option<&str>, sender: Option<Sender<'_>>, url: &Url) -> bool {
        if !self.host_matches(host) {
            return false;
        }

        if let Some(path) = &self.path
            && !path.matches(url.path())
        {
            return false;
        }

        let sender = sender.map(|sender| (sender.nick, sender.username, sender.hostname));

        Self::field_matches(self.nickname.as_ref(), sender.map(|s| s.0))
            && Self::field_matches(self.username.as_ref(), sender.map(|s| s.1))
            && Self::field_matches(self.hostname.as_ref(), sender.map(|s| s.2))
    }
}

/// The filters of a single channel (or of all channels), indexed by URL host.
#[derive(Debug, Default)]
struct HostIndex {
    /// Filters with an exact host pattern, keyed by the lowercased host.
    exact: HashMap<String, Vec<Entry>>,
    /// Filters with a wildcard host pattern.
    wildcard: Vec<Entry>,
    /// Filters without a host pattern.
    any_host: Vec<Entry>,
}

impl HostIndex {
    /// Inserts `entry` into the bucket its host pattern routes to.
    fn insert(&mut self, entry: Entry) {
        match &entry.host {
            Some(HostMatcher::Exact(host)) => {
                self.exact.entry(host.clone()).or_default().push(entry);
            }
            Some(HostMatcher::Wild(_)) => self.wildcard.push(entry),
            None => self.any_host.push(entry),
        }
    }

    /// Removes the entries with the given filter ids.
    fn remove_ids(&mut self, ids: &[i32]) {
        for entries in self.exact.values_mut() {
            entries.retain(|entry| !ids.contains(&entry.filter.id));
        }

        self.wildcard
            .retain(|entry| !ids.contains(&entry.filter.id));
        self.any_host
            .retain(|entry| !ids.contains(&entry.filter.id));
    }

    /// All filters in the bucket, in insertion order.
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
            .map(Vec::as_slice)
            .unwrap_or_default();

        exact
            .iter()
            .chain(&self.wildcard)
            .chain(&self.any_host)
            .any(|entry| entry.matches(host, sender, url))
    }
}

/// All filters, indexed by channel.
///
/// The index is keyed by the lowercased channel name; the `None` key holds the filters that
/// apply to all channels. A lookup consults both the channel's bucket and the all-channels
/// bucket, and never touches the filters of other channels.
#[derive(Debug, Default)]
pub(super) struct FilterIndex {
    channels: HashMap<Option<String>, HostIndex>,
}

impl FilterIndex {
    /// Inserts `filter` into the index.
    pub(super) fn insert(&mut self, filter: Filter) {
        let channel = filter.channel.as_deref().map(str::to_lowercase);
        let entry = Entry::new(filter);

        self.channels.entry(channel).or_default().insert(entry);
    }

    /// Removes the filters with the given ids.
    pub(super) fn remove_ids(&mut self, ids: &[i32]) {
        self.channels
            .values_mut()
            .for_each(|bucket| bucket.remove_ids(ids));
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

    /// Whether any filter applies to `channel` and matches `sender` and `url`.
    pub(super) fn matches(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        let host = url.host_str().map(str::to_lowercase);
        let key = channel.to_lowercase();

        for bucket in [self.channels.get(&Some(key)), self.channels.get(&None)] {
            let Some(bucket) = bucket else {
                continue;
            };

            if bucket.matches(host.as_deref(), sender, url) {
                trace!(%channel, %url, "message matches a filter");

                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::NewFilter;
    use super::*;

    fn filter(id: i32, criteria: NewFilter) -> Filter {
        Filter {
            id,
            channel: criteria.channel,
            host: criteria.host,
            path: criteria.path,
            nickname: criteria.nickname,
            username: criteria.username,
            hostname: criteria.hostname,
            created_by: "smoke".into(),
            created_at: sqlx::types::chrono::Utc::now(),
        }
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

    #[test]
    fn indexes_by_exact_host() {
        let mut index = FilterIndex::default();

        index.insert(filter(1, new_filter(None, Some("imdb.com"))));

        assert!(matches(&index, "#chan", "https://imdb.com/title/tt1"));
        assert!(!matches(&index, "#chan", "https://www.imdb.com/title/tt1"));
        assert!(!matches(&index, "#chan", "https://dr.dk"));
    }

    #[test]
    fn indexes_wildcard_hosts() {
        let mut index = FilterIndex::default();

        index.insert(filter(1, new_filter(None, Some("*.com"))));

        assert!(matches(&index, "#chan", "https://reuters.com/some-article"));
        assert!(!matches(&index, "#chan", "https://dr.dk/some-article"));
    }

    #[test]
    fn host_patterns_are_case_insensitive() {
        let mut index = FilterIndex::default();

        index.insert(filter(1, new_filter(None, Some("IMDB.COM"))));

        assert!(matches(&index, "#chan", "https://imdb.com/title/tt1"));

        let mut index = FilterIndex::default();

        index.insert(filter(1, new_filter(None, Some("imdb.com"))));

        assert!(matches(&index, "#chan", "https://IMDB.com/title/tt1"));
    }

    #[test]
    fn scopes_filters_to_channels() {
        let mut index = FilterIndex::default();

        index.insert(filter(1, new_filter(Some("#foo"), Some("dr.dk"))));

        assert!(matches(&index, "#FOO", "https://dr.dk"));
        assert!(!matches(&index, "#bar", "https://dr.dk"));
    }

    #[test]
    fn matches_all_channel_filters_in_any_channel() {
        let mut index = FilterIndex::default();

        index.insert(filter(1, new_filter(None, Some("dr.dk"))));

        assert!(matches(&index, "#foo", "https://dr.dk"));
        assert!(matches(&index, "#bar", "https://dr.dk"));
    }

    #[test]
    fn sender_criteria_require_a_sender() {
        let mut index = FilterIndex::default();

        index.insert(filter(
            1,
            NewFilter {
                username: Some("*other".into()),
                ..new_filter(None, Some("imdb.com"))
            },
        ));

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
    fn combines_criteria_with_and() {
        let mut index = FilterIndex::default();

        index.insert(filter(
            1,
            NewFilter {
                nickname: Some("mk".into()),
                ..new_filter(None, Some("dr.dk"))
            },
        ));

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
        let mut index = FilterIndex::default();

        index.insert(filter(
            1,
            NewFilter {
                path: Some("/title/*".into()),
                ..new_filter(None, Some("imdb.com"))
            },
        ));

        assert!(matches(&index, "#chan", "https://imdb.com/title/tt1375666"));
        assert!(matches(
            &index,
            "#chan",
            "https://imdb.com/title/tt1375666/mediaviewer/rm1"
        ));
        assert!(!matches(&index, "#chan", "https://imdb.com/name/nm0186505"));

        let mut index = FilterIndex::default();

        index.insert(filter(
            1,
            NewFilter {
                path: Some("/Title/*".into()),
                ..new_filter(None, None)
            },
        ));

        assert!(!matches(&index, "#chan", "https://maero.dk/title/x"));
        assert!(matches(&index, "#chan", "https://maero.dk/Title/x"));
    }

    #[test]
    fn matches_urls_without_a_host_against_hostless_filters() {
        let mut index = FilterIndex::default();

        index.insert(filter(1, new_filter(None, None)));

        assert!(index.matches(
            "#chan",
            None,
            &Url::parse("mailto:someone@example.com").unwrap()
        ));

        let mut index = FilterIndex::default();

        index.insert(filter(1, new_filter(None, Some("*"))));

        assert!(index.matches(
            "#chan",
            None,
            &Url::parse("mailto:someone@example.com").unwrap()
        ));

        let mut index = FilterIndex::default();

        index.insert(filter(1, new_filter(None, Some("example.com"))));

        assert!(!index.matches(
            "#chan",
            None,
            &Url::parse("mailto:someone@example.com").unwrap()
        ));
    }

    #[test]
    fn removes_filters_by_id() {
        let mut index = FilterIndex::default();

        index.insert(filter(1, new_filter(None, Some("imdb.com"))));
        index.insert(filter(2, new_filter(None, Some("dr.dk"))));

        index.remove_ids(&[1]);

        assert!(!matches(&index, "#chan", "https://imdb.com/title/tt1"));
        assert!(matches(&index, "#chan", "https://dr.dk"));
    }

    #[test]
    fn lists_all_filters() {
        let mut index = FilterIndex::default();

        index.insert(filter(1, new_filter(Some("#foo"), Some("imdb.com"))));
        index.insert(filter(2, new_filter(None, Some("dr.dk"))));

        let ids: Vec<_> = index.filters().iter().map(|filter| filter.id).collect();

        assert_eq!(ids, [1, 2]);
    }
}
