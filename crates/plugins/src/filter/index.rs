//! In-memory filter index with precompiled wildcard matchers.
//!
//! Filters are indexed by channel and, within each channel, by URL host, so the hot lookup path
//! never scans every filter: a message resolves its channel bucket (and the all-channels
//! bucket), and within it only the filters indexed for the URL's exact host plus the wildcard
//! and host-agnostic entries are considered. All wildcard patterns are compiled once when a
//! filter is inserted, not on every check.

use std::collections::{HashMap, HashSet};

use tracing::trace;
use url::Url;

use super::{
    model::{CompiledCriteria, Filter},
    pattern::{Case, Pattern, has_wildcards, non_blank},
};
use zeta_plugin::Sender;

/// The compiled host pattern of a filter.
#[derive(Debug)]
enum HostMatcher {
    /// An exact host, matched for equality.
    Exact(String),
    /// A wildcard host pattern.
    Wild(Pattern),
}

impl HostMatcher {
    /// Compiles an optional host pattern.
    fn parse(host: Option<&str>) -> Option<Self> {
        let host = non_blank(host)?;

        Some(if has_wildcards(host) {
            Self::Wild(Pattern::new(host, Case::Insensitive))
        } else {
            Self::Exact(host.to_lowercase())
        })
    }

    /// Whether the host pattern matches `host` (already lowercased). Hostless URLs match
    /// wildcard patterns as if their host were empty, so `*` matches every URL.
    fn matches(&self, host: Option<&str>) -> bool {
        match self {
            Self::Exact(expected) => host == Some(expected.as_str()),
            Self::Wild(pattern) => pattern.matches(host.unwrap_or("")),
        }
    }
}

/// A filter with its wildcard patterns precompiled for matching.
#[derive(Debug)]
struct Entry {
    /// The raw filter.
    filter: Filter,
    /// The compiled host pattern, if any.
    host: Option<HostMatcher>,
    /// The compiled URL path pattern, if any.
    path: Option<Pattern>,
    /// The compiled sender nickname pattern, if any.
    nickname: Option<Pattern>,
    /// The compiled sender username pattern, if any.
    username: Option<Pattern>,
    /// The compiled sender hostname pattern, if any.
    hostname: Option<Pattern>,
}

impl Entry {
    /// Compiles `filter` into an entry.
    fn new(filter: Filter) -> Self {
        Self {
            host: HostMatcher::parse(filter.host.as_deref()),
            path: Pattern::parse(filter.path.as_deref(), Case::Sensitive),
            nickname: Pattern::parse(filter.nickname.as_deref(), Case::Insensitive),
            username: Pattern::parse(filter.username.as_deref(), Case::Insensitive),
            hostname: Pattern::parse(filter.hostname.as_deref(), Case::Insensitive),
            filter,
        }
    }

    /// Whether the entry's host pattern matches `host`, or is unset.
    fn host_matches(&self, host: Option<&str>) -> bool {
        self.host
            .as_ref()
            .is_none_or(|pattern| pattern.matches(host))
    }

    /// Whether a compiled pattern matches `value`, or is unset.
    fn field_matches(pattern: Option<&Pattern>, value: &str) -> bool {
        pattern.is_none_or(|pattern| pattern.matches(value))
    }

    /// Whether the entry has a sender criterion.
    const fn has_sender_criteria(&self) -> bool {
        self.nickname.is_some() || self.username.is_some() || self.hostname.is_some()
    }

    /// Whether the entry has URL criteria.
    const fn has_url_criteria(&self) -> bool {
        self.host.is_some() || self.path.is_some()
    }

    /// Whether the entry's sender criteria all match `sender`, or are all unset.
    fn sender_matches(&self, sender: Option<Sender<'_>>) -> bool {
        let Some(sender) = sender else {
            // A set sender criterion needs a sender to match against.
            return !self.has_sender_criteria();
        };

        Self::field_matches(self.nickname.as_ref(), sender.nick)
            && Self::field_matches(self.username.as_ref(), sender.username)
            && Self::field_matches(self.hostname.as_ref(), sender.hostname)
    }

    /// Whether the entry matches `sender` and `url`. The host already routed the entry here:
    /// exact entries are matched by bucket and host-agnostic ones have no host pattern, so only
    /// wildcard entries need the host check.
    fn matches(&self, sender: Option<Sender<'_>>, url: &Url) -> bool {
        let path_matches = self
            .path
            .as_ref()
            .is_none_or(|path| path.matches(url.path()));

        path_matches && self.sender_matches(sender)
    }

    /// Whether the entry ignores `sender` entirely: it has no URL criteria and at least one
    /// sender criterion, which matches. The sender-criteria requirement keeps a criteria-less
    /// filter — which the command cannot create — from ignoring everyone.
    fn ignores(&self, sender: Option<Sender<'_>>) -> bool {
        !self.has_url_criteria() && self.has_sender_criteria() && self.sender_matches(sender)
    }
}

/// The filters of a single channel (or of all channels), indexed by URL host.
#[derive(Debug, Default)]
struct ChannelBucket {
    /// Filters with an exact host pattern, keyed by the lowercased host.
    exact: HashMap<String, Vec<Entry>>,
    /// Filters with a wildcard host pattern.
    wildcard: Vec<Entry>,
    /// Filters without a host pattern.
    any_host: Vec<Entry>,
}

impl ChannelBucket {
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

    /// Removes the entries with the given filter ids, dropping host buckets that become empty.
    fn remove_ids(&mut self, ids: &HashSet<i32>) {
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

    /// All filters in the bucket, in insertion order.
    fn filters(&self) -> impl Iterator<Item = &Filter> {
        self.exact
            .values()
            .flatten()
            .chain(&self.wildcard)
            .chain(&self.any_host)
            .map(|entry| &entry.filter)
    }

    /// The filter with `id`, if the bucket holds it.
    fn get(&self, id: i32) -> Option<&Filter> {
        self.filters().find(|filter| filter.id == id)
    }

    /// Whether any entry in the bucket matches `host`, `sender` and `url`. Exact and
    /// host-agnostic entries are already routed by the host they were looked up with; only
    /// wildcard entries are matched against it.
    fn matches(&self, host: Option<&str>, sender: Option<Sender<'_>>, url: &Url) -> bool {
        let exact = host
            .and_then(|host| self.exact.get(host))
            .map_or_default(Vec::as_slice);

        exact
            .iter()
            .chain(&self.any_host)
            .any(|entry| entry.matches(sender, url))
            || self
                .wildcard
                .iter()
                .any(|entry| entry.host_matches(host) && entry.matches(sender, url))
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
#[derive(Debug, Default)]
pub(super) struct FilterIndex {
    /// The buckets of the individual channels, and the all-channels bucket under `None`.
    channels: HashMap<Option<String>, ChannelBucket>,
}

impl FilterIndex {
    /// Inserts `filter` into the index.
    pub(super) fn insert(&mut self, filter: Filter) {
        let channel = filter.channel.as_deref().map(str::to_lowercase);

        self.channels
            .entry(channel)
            .or_default()
            .insert(Entry::new(filter));
    }

    /// Removes the filters with the given ids, dropping channel buckets that become empty.
    pub(super) fn remove_ids(&mut self, ids: &[i32]) {
        let ids: HashSet<i32> = ids.iter().copied().collect();

        self.channels.retain(|_, bucket| {
            bucket.remove_ids(&ids);

            !bucket.is_empty()
        });
    }

    /// The number of filters in the index.
    pub(super) fn len(&self) -> usize {
        self.channels
            .values()
            .flat_map(ChannelBucket::filters)
            .count()
    }

    /// Whether the index holds no filters at all.
    pub(super) fn is_empty(&self) -> bool {
        // Empty buckets are dropped on removal, so an empty map means no filters.
        self.channels.is_empty()
    }

    /// All filters in the index, ordered by id.
    pub(super) fn filters(&self) -> Vec<Filter> {
        self.matching(|_| true)
    }

    /// The filter with `id`, if any.
    pub(super) fn get(&self, id: i32) -> Option<Filter> {
        self.channels
            .values()
            .find_map(|bucket| bucket.get(id))
            .cloned()
    }

    /// The filters matching `criteria`, ordered by id.
    pub(super) fn select(&self, criteria: &CompiledCriteria) -> Vec<Filter> {
        self.matching(|filter| criteria.matches(filter))
    }

    /// The filters matching `predicate`, ordered by id.
    fn matching(&self, predicate: impl Fn(&Filter) -> bool) -> Vec<Filter> {
        let mut filters: Vec<Filter> = self
            .channels
            .values()
            .flat_map(ChannelBucket::filters)
            .filter(|filter| predicate(filter))
            .cloned()
            .collect();

        filters.sort_unstable_by_key(|filter| filter.id);

        filters
    }

    /// The buckets a lookup in `channel` consults: the channel's own filters and the filters
    /// applying to all channels.
    fn buckets(&self, channel: &str) -> impl Iterator<Item = &ChannelBucket> {
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
        let matched = self
            .buckets(channel)
            .any(|bucket| bucket.matches(host.as_deref(), sender, url));

        if matched {
            trace!(%channel, %url, "message matches a filter");
        }

        matched
    }

    /// Whether any filter applies to `channel` that ignores `sender` entirely.
    pub(super) fn ignores(&self, channel: &str, sender: Option<Sender<'_>>) -> bool {
        if self.is_empty() {
            return false;
        }

        let ignored = self.buckets(channel).any(|bucket| bucket.ignores(sender));

        if ignored {
            trace!(%channel, "sender matches an ignore filter");
        }

        ignored
    }
}

impl FromIterator<Filter> for FilterIndex {
    fn from_iter<T: IntoIterator<Item = Filter>>(filters: T) -> Self {
        let mut index = Self::default();

        for filter in filters {
            index.insert(filter);
        }

        index
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::{Criteria, NewFilter};
    use super::*;

    fn filter(id: i32, new: NewFilter) -> Filter {
        Filter {
            id,
            channel: new.channel,
            host: new.host,
            path: new.path,
            nickname: new.nickname,
            username: new.username,
            hostname: new.hostname,
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

    /// Returns an index pre-populated with the given filters, numbered from 1.
    fn index_with(criteria: &[NewFilter]) -> FilterIndex {
        criteria
            .iter()
            .enumerate()
            .map(|(position, criteria)| {
                let id = i32::try_from(position).expect("fewer filters than `i32` allows") + 1;

                filter(id, criteria.clone())
            })
            .collect()
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
    fn indexes_by_exact_host() {
        let index = index_with(&[new_filter(None, Some("imdb.com"))]);

        assert!(matches(&index, "#chan", "https://imdb.com/title/tt1"));
        assert!(!matches(&index, "#chan", "https://www.imdb.com/title/tt1"));
        assert!(!matches(&index, "#chan", "https://dr.dk"));
    }

    #[test]
    fn indexes_wildcard_hosts() {
        let index = index_with(&[new_filter(None, Some("*.com"))]);

        assert!(matches(&index, "#chan", "https://reuters.com/some-article"));
        assert!(!matches(&index, "#chan", "https://dr.dk/some-article"));
    }

    #[test]
    fn host_patterns_are_case_insensitive() {
        let upper = index_with(&[new_filter(None, Some("IMDB.COM"))]);

        assert!(matches(&upper, "#chan", "https://imdb.com/title/tt1"));

        let lower = index_with(&[new_filter(None, Some("imdb.com"))]);

        assert!(matches(&lower, "#chan", "https://IMDB.com/title/tt1"));
    }

    #[test]
    fn scopes_filters_to_channels() {
        let index = index_with(&[new_filter(Some("#foo"), Some("dr.dk"))]);

        assert!(matches(&index, "#FOO", "https://dr.dk"));
        assert!(!matches(&index, "#bar", "https://dr.dk"));
    }

    #[test]
    fn matches_all_channel_filters_in_any_channel() {
        let index = index_with(&[new_filter(None, Some("dr.dk"))]);

        assert!(matches(&index, "#foo", "https://dr.dk"));
        assert!(matches(&index, "#bar", "https://dr.dk"));
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
    fn filters_with_url_criteria_do_not_ignore_senders() {
        let host = index_with(&[NewFilter {
            nickname: Some("*bot".into()),
            ..new_filter(None, Some("imdb.com"))
        }]);
        let path = index_with(&[NewFilter {
            nickname: Some("*bot".into()),
            path: Some("/title/*".into()),
            ..new_filter(None, None)
        }]);

        assert!(!host.ignores("#chan", Some(Sender::new("a_bot", "user", "host.example"))));
        assert!(!path.ignores("#chan", Some(Sender::new("a_bot", "user", "host.example"))));
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
    }

    #[test]
    fn removing_the_last_filter_empties_the_index() {
        let mut index = index_with(&[new_filter(None, Some("imdb.com"))]);

        index.remove_ids(&[1]);

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

    #[test]
    fn gets_a_filter_by_id() {
        let index = index_with(&[new_filter(None, Some("imdb.com"))]);

        assert_eq!(index.get(1).map(|filter| filter.id), Some(1));
        assert!(index.get(2).is_none());
    }

    #[test]
    fn selects_filters_matching_criteria() {
        let index = index_with(&[
            new_filter(Some("#foo"), Some("imdb.com")),
            new_filter(Some("#bar"), Some("dr.dk")),
        ]);
        let criteria = Criteria {
            host: Some("*.com".into()),
            ..Criteria::default()
        };

        let selected = index.select(&criteria.compile());

        assert_eq!(
            selected.iter().map(|filter| filter.id).collect::<Vec<_>>(),
            [1]
        );
    }
}
