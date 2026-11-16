//! Parsing and construction of Instagram URLs.
//!
//! The module is split by concern: `classify` recognises the hosts and path shapes this plugin
//! handles and identifies what they point at, `canonical` builds the canonical URLs used for the
//! metadata and mirror requests, and `tests` covers both. Every item is re-exported here, so
//! `crate::urls::{…}` stays the import path for the rest of the plugin.

mod canonical;
mod classify;
#[cfg(test)]
mod tests;

pub(crate) use self::canonical::{media_url, story_url};
pub(crate) use self::classify::{InstagramLink, MediaKind, URL_HOSTS, parse_instagram_url};
