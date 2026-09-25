//! The data model of the unwall plugin.

use sqlx::{
    prelude::FromRow,
    types::chrono::{DateTime, Utc},
};

/// An unwalled URL, as cached in the database.
///
/// The reader link is derivable from the article URL; caching it keeps the replied link stable
/// across configuration changes of [`reader_base`](super::Settings::reader_base).
#[derive(Debug, FromRow, Clone, PartialEq, Eq)]
pub struct CachedUrl {
    /// The database id of the cached URL.
    pub id: i32,
    /// The article URL as posted, the cache key.
    pub url: String,
    /// The host of the article URL.
    pub host: String,
    /// The unwall.app reader link replied with.
    pub unwall_url: String,
    /// When the URL was first unwalled.
    pub created_at: DateTime<Utc>,
}

/// A site unwalled in addition to the tested domains, as stored in the database.
#[derive(Debug, FromRow, Clone, PartialEq, Eq)]
pub struct Site {
    /// The database id of the site.
    pub id: i32,
    /// The host, or a `*.domain` wildcard.
    pub host: String,
    /// The nickname of the admin who added the site.
    pub nickname: String,
    /// When the site was added.
    pub created_at: DateTime<Utc>,
}

/// The removal of a tested domain, as stored in the database.
///
/// A tombstone, so the tested domains can stay a static-and-refreshed list while admins
/// exclude individual domains from coverage; adding the domain back deletes the tombstone.
#[derive(Debug, FromRow, Clone, PartialEq, Eq)]
pub struct SiteRemoval {
    /// The database id of the removal.
    pub id: i32,
    /// The removed host, or a `*.domain` wildcard.
    pub host: String,
    /// The nickname of the admin who removed the site.
    pub nickname: String,
    /// When the site was removed.
    pub created_at: DateTime<Utc>,
}

/// An initial unwall fetch to attribute, recorded for every submission.
#[derive(Debug, Clone)]
pub struct NewFetch {
    /// The article URL, the cache key.
    pub url: String,
    /// The host of the article URL.
    pub host: String,
    /// The nickname of the user who posted the link.
    pub nickname: String,
    /// The username (ident) of the user who posted the link.
    pub username: String,
    /// The hostname of the user who posted the link.
    pub hostname: String,
    /// The channel the link was posted in.
    pub channel: String,
    /// The identifier of the network the link was posted on.
    pub network_id: String,
}

/// Overall statistics about the unwalled URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Statistics {
    /// The number of article URLs unwalled.
    pub urls: i64,
    /// The number of article URLs unwalled today.
    pub urls_today: i64,
    /// The number of distinct hosts unwalled.
    pub hosts: i64,
    /// The number of initial fetches, attributed to a user.
    pub fetches: i64,
    /// The number of initial fetches today.
    pub fetches_today: i64,
    /// The number of distinct users who triggered an initial fetch.
    pub users: i64,
}

/// The statistics of a single host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostStatistics {
    /// The number of links unwalled for the host.
    pub urls: i64,
    /// The number of links unwalled for the host today.
    pub urls_today: i64,
    /// The number of distinct users who triggered a fetch for the host.
    pub users: i64,
    /// When a link for the host was last unwalled.
    pub latest: Option<DateTime<Utc>>,
}
