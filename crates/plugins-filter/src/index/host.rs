//! Host matching: the compiled host matcher and the host-indexed channel bucket.

use std::sync::Arc;

use url::Url;
use zeta_plugin::Sender;

use super::IndexMap;
use super::entry::Entry;
use crate::model::{Filter, Pattern, is_wildcard};

/// The compiled host pattern of a filter.
#[derive(Debug, Clone)]
pub(super) enum HostMatcher {
    /// An exact host, matched for equality.
    Exact(String),
    /// An ASCII `*.suffix` host pattern, matched against the host tail.
    Suffix(String),
    /// A wildcard host pattern.
    Wild(Pattern),
}

/// Returns the literal suffix of an ASCII `*.suffix` host pattern.
///
/// The suffix must be non-empty and free of further wildcards; every other wildcard host
/// pattern keeps the general [`HostMatcher::Wild`] matcher. This applies to URL host patterns
/// only: sender hostname patterns always use [`Pattern`].
pub(super) fn host_suffix(host: &str) -> Option<&str> {
    let suffix = host.strip_prefix("*.")?;

    (!suffix.is_empty() && suffix.is_ascii() && !is_wildcard(suffix)).then_some(suffix)
}

/// The filters of a single channel (or of all channels), indexed by URL host.
#[derive(Debug, Default, Clone)]
pub(super) struct HostIndex {
    /// Filters with an exact host pattern, keyed by the lowercased host.
    exact: IndexMap<String, Vec<Arc<Entry>>>,
    /// Filters with a wildcard or `*.suffix` host pattern.
    wildcard: Vec<Arc<Entry>>,
    /// Filters without a host pattern.
    any_host: Vec<Arc<Entry>>,
}

impl HostIndex {
    /// Inserts `entry` into the bucket its host pattern routes to.
    pub(super) fn insert(&mut self, entry: Arc<Entry>) {
        match &entry.host {
            Some(HostMatcher::Exact(host)) => {
                self.exact.entry(host.clone()).or_default().push(entry);
            }
            Some(HostMatcher::Wild(_) | HostMatcher::Suffix(_)) => self.wildcard.push(entry),
            None => self.any_host.push(entry),
        }
    }

    /// Removes the entries with the given filter ids, dropping host buckets that become empty,
    /// and returns how many entries were removed.
    pub(super) fn remove_ids(&mut self, ids: &[i32]) -> usize {
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
    pub(super) fn is_empty(&self) -> bool {
        self.exact.is_empty() && self.wildcard.is_empty() && self.any_host.is_empty()
    }

    /// All filters in the bucket, in unspecified order.
    pub(super) fn filters(&self) -> impl Iterator<Item = &Filter> {
        self.exact
            .values()
            .flatten()
            .chain(&self.wildcard)
            .chain(&self.any_host)
            .map(|entry| &entry.filter)
    }

    /// Whether any entry in the bucket matches `host`, `sender` and `url`.
    pub(super) fn matches(
        &self,
        host: Option<&str>,
        sender: Option<Sender<'_>>,
        url: &Url,
    ) -> bool {
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
    pub(super) fn ignores(&self, sender: Option<Sender<'_>>) -> bool {
        self.any_host.iter().any(|entry| entry.ignores(sender))
    }
}
