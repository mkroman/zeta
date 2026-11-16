//! A filter with its URL and sender patterns compiled once for repeated matching.

use url::Url;
use zeta_plugin::Sender;

use super::host::{HostMatcher, host_suffix};
use crate::model::{Case, Filter, Pattern, is_wildcard};

/// Returns `pattern` trimmed, or `None` when it is missing or blank.
fn non_blank(pattern: Option<&str>) -> Option<&str> {
    pattern.map(str::trim).filter(|pattern| !pattern.is_empty())
}

/// A filter with its wildcard patterns precompiled for matching.
///
/// Shared behind an [`Arc`](std::sync::Arc) so cloning the index on add/delete does not copy
/// every filter and matcher.
#[derive(Debug)]
pub(super) struct Entry {
    /// The raw filter.
    pub(super) filter: Filter,
    /// The compiled host pattern, if any.
    pub(super) host: Option<HostMatcher>,
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
    pub(super) fn new(filter: Filter) -> Self {
        let host = non_blank(filter.host.as_deref()).map(|host| {
            host_suffix(host).map_or_else(
                || {
                    if is_wildcard(host) {
                        HostMatcher::Wild(Pattern::compile(Some(host), Case::Insensitive))
                    } else {
                        HostMatcher::Exact(host.to_lowercase())
                    }
                },
                |suffix| HostMatcher::Suffix(suffix.to_ascii_lowercase()),
            )
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

    /// Whether the host pattern of the entry matches `host` (already lowercased).
    ///
    /// Hostless URLs match wildcard patterns as if their host were empty, so `*` matches every
    /// URL; exact and suffix patterns never match them.
    fn host_matches(&self, host: Option<&str>) -> bool {
        match &self.host {
            None => true,
            Some(HostMatcher::Exact(expected)) => host == Some(expected.as_str()),
            Some(HostMatcher::Suffix(suffix)) => host.is_some_and(|host| {
                host.len() > suffix.len()
                    && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
                    && host[host.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
            }),
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
    /// Only wildcard and suffix entries need this: exact entries are routed by their host and
    /// hostless entries pass the host check unconditionally, so both use
    /// [`Entry::matches_after_host`].
    pub(super) fn matches(
        &self,
        host: Option<&str>,
        sender: Option<Sender<'_>>,
        url: &Url,
    ) -> bool {
        self.host_matches(host) && self.matches_after_host(sender, url)
    }

    /// Whether the entry matches `sender` and `url` once its host is already known to match.
    pub(super) fn matches_after_host(&self, sender: Option<Sender<'_>>, url: &Url) -> bool {
        // The dispatcher consults `ignores` before filtering any URL, so a sender-only entry
        // whose sender matches never reaches this point; one whose sender does not match
        // cannot match the message either.
        if self.sender_only() {
            return false;
        }

        self.path.matches(Some(url.path())) && self.sender_matches(sender)
    }

    /// Whether the entry has no URL criteria (host and path unset) and at least one sender
    /// criterion.
    ///
    /// These entries are handled entirely by [`Entry::ignores`]. A criteria-less filter is not
    /// sender-only and keeps matching every URL.
    pub(super) const fn sender_only(&self) -> bool {
        self.host.is_none()
            && self.path.is_unset()
            && (!self.nickname.is_unset() || !self.username.is_unset() || !self.hostname.is_unset())
    }

    /// Whether the entry ignores `sender` entirely: it is sender-only and its sender criteria
    /// match.
    pub(super) fn ignores(&self, sender: Option<Sender<'_>>) -> bool {
        self.sender_only() && self.sender_matches(sender)
    }
}
