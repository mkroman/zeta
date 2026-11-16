//! In-memory filter index with precompiled wildcard matchers.
//!
//! Filters are indexed by channel and, within each channel, by URL host, so the hot lookup path
//! never scans every filter: a message resolves its channel bucket (and the all-channels
//! bucket), and within it only the filters indexed for the URL's exact host plus the wildcard
//! and host-agnostic entries are considered. All wildcard patterns are compiled once when a
//! filter is inserted, not on every check.
//!
//! The module is split by concern: `entry` holds one filter with its patterns compiled for
//! matching, `host` the compiled host matchers and the host-indexed channel bucket, and this
//! module the channel-keyed index and its lookups.

mod entry;
mod host;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::Arc;

use tracing::trace;
use url::Url;
use zeta_plugin::Sender;
use zeta_url::RedactedUrlRef;

use self::entry::Entry;
use self::host::HostIndex;
use super::model::Filter;

/// A hash map for the hot filter-index lookups, keyed by lowercased channel and host names.
///
/// `foldhash::fast` hashes these short keys faster than the std hasher. It is not
/// DoS-resistant: every key is inserted from operator-configured filters, and URL text is only
/// ever used to look an existing key up. If untrusted keys are ever inserted, this must switch
/// to `foldhash::quality` or the std `RandomState`.
type IndexMap<K, V> = HashMap<K, V, foldhash::fast::RandomState>;

/// The stack buffer capacity, in bytes, for lowercased ASCII channel keys.
///
/// RFC 2812 caps channel names at 50 bytes and common servers allow up to 64, so every valid
/// name fits on the hot path; longer names fall back to [`str::to_lowercase`].
const CHANNEL_KEY_CAPACITY: usize = 64;

/// Calls `f` with the lowercased index key of `channel`.
///
/// Lowercase ASCII channels, the common case, are passed through without allocating or
/// initializing a stack buffer. Mixed-case ASCII names are lowered into a stack buffer that is
/// only created on that path, so they do not allocate either. Non-ASCII names (and names longer
/// than [`CHANNEL_KEY_CAPACITY`]) keep [`str::to_lowercase`], which preserves the exact Unicode
/// lowercase semantics the index is keyed by.
fn with_channel_key<R>(channel: &str, f: impl FnOnce(&str) -> R) -> R {
    // Names beyond the IRC limit keep the allocating fallback so the common path pays nothing.
    if channel.len() > CHANNEL_KEY_CAPACITY {
        return f(&channel.to_lowercase());
    }

    if channel
        .bytes()
        .all(|byte| byte.is_ascii() && !byte.is_ascii_uppercase())
    {
        return f(channel);
    }

    if channel.is_ascii() {
        let mut buffer = [0u8; CHANNEL_KEY_CAPACITY];
        let key = &mut buffer[..channel.len()];
        key.copy_from_slice(channel.as_bytes());
        key.make_ascii_lowercase();

        return f(std::str::from_utf8(key).expect("ASCII is valid UTF-8"));
    }

    f(&channel.to_lowercase())
}

/// All filters, indexed by channel.
///
/// The index is keyed by the lowercased channel name; [`all_channels`](Self::all_channels)
/// holds the filters that apply to all channels. A lookup consults both, and never touches the
/// filters of other channels.
#[derive(Debug, Default, Clone)]
pub(crate) struct FilterIndex {
    /// The channel buckets, keyed by lowercased channel name.
    channels: IndexMap<String, HostIndex>,
    /// The filters that apply to all channels.
    all_channels: HostIndex,
    /// The number of filters in the index, kept exact by [`FilterIndex::insert`] and
    /// [`FilterIndex::remove_ids`] so [`FilterIndex::len`] and [`FilterIndex::is_empty`] are
    /// constant-time.
    count: usize,
    /// Whether any inserted filter was sender-only.
    ///
    /// A monotone fast-path flag mirroring `EventIndex::command_initials`: [`FilterIndex::insert`]
    /// only ever sets it, and [`FilterIndex::remove_ids`] leaves it set. A stale `true` after a
    /// deletion costs a bucket lookup, never a wrong answer.
    has_sender_only: bool,
}

impl FilterIndex {
    /// Inserts `filter` into the index.
    pub(super) fn insert(&mut self, filter: Filter) {
        // Resolve the bucket through the stack-lowercased key first, so only the first filter
        // of a channel allocates its key. The borrow ends before `filter` is moved into the
        // entry.
        let existing = filter
            .channel
            .as_deref()
            .and_then(|channel| with_channel_key(channel, |key| self.channels.get_mut(key)));

        let entry = Arc::new(Entry::new(filter));

        self.has_sender_only |= entry.sender_only();

        if let Some(bucket) = existing {
            bucket.insert(entry);
        } else if let Some(channel) = entry.filter.channel.as_deref() {
            // The stack helper's lowercasing is exactly `str::to_lowercase` for a new key.
            let key = channel.to_lowercase();
            let mut bucket = HostIndex::default();
            bucket.insert(entry);
            self.channels.insert(key, bucket);
        } else {
            self.all_channels.insert(entry);
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
        self.sorted_filters().into_iter().cloned().collect()
    }

    /// Returns the filter with `id`, without ordering or cloning the whole index.
    pub(super) fn find(&self, id: i32) -> Option<&Filter> {
        self.channels
            .values()
            .chain(std::iter::once(&self.all_channels))
            .flat_map(HostIndex::filters)
            .find(|filter| filter.id == id)
    }

    /// All filters in the index as references, ordered by id.
    pub(super) fn sorted_filters(&self) -> Vec<&Filter> {
        let mut filters: Vec<&Filter> = Vec::with_capacity(self.count);
        filters.extend(
            self.channels
                .values()
                .chain(std::iter::once(&self.all_channels))
                .flat_map(HostIndex::filters),
        );

        filters.sort_unstable_by_key(|filter| filter.id);

        filters
    }

    /// The buckets a lookup with the lowercased channel `key` consults: the channel's own
    /// filters and the filters applying to all channels.
    fn buckets(&self, key: &str) -> impl Iterator<Item = &HostIndex> {
        let all_channels = (!self.all_channels.is_empty()).then_some(&self.all_channels);

        [self.channels.get(key), all_channels].into_iter().flatten()
    }

    /// Whether any filter applies to `channel` and matches `sender` and `url`.
    pub(super) fn matches(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        if self.is_empty() {
            return false;
        }

        // `Url::host_str` is already lowercased for http(s) URLs, so it is used as-is.
        let host = url.host_str();

        if with_channel_key(channel, |key| {
            self.buckets(key)
                .any(|bucket| bucket.matches(host, sender, url))
        }) {
            trace!(url.full = %RedactedUrlRef::from_url(url), "message matches a filter");

            true
        } else {
            false
        }
    }

    /// Whether any filter applies to `channel` that ignores `sender` entirely.
    pub(super) fn ignores(&self, channel: &str, sender: Option<Sender<'_>>) -> bool {
        if !self.has_sender_only {
            return false;
        }

        if with_channel_key(channel, |key| {
            self.buckets(key).any(|bucket| bucket.ignores(sender))
        }) {
            trace!("sender matches an ignore filter");

            true
        } else {
            false
        }
    }
}
