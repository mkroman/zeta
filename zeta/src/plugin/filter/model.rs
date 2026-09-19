//! The data model of the filter plugin.

use sqlx::{
    prelude::FromRow,
    types::chrono::{DateTime, Utc},
};

/// A filter, as stored in the database.
///
/// Every criterion is optional; a filter matches a message when all of its set criteria match —
/// the channel the message was posted in, the host and path of a URL in it, and the nickname,
/// username (ident), and hostname of its sender. Patterns may contain `*` and `?` wildcards;
/// paths are matched case-sensitively, everything else case-insensitively.
// Some fields are only populated from the database and never read.
#[derive(Debug, FromRow, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct Filter {
    /// The database id of the filter.
    pub id: i32,
    /// The channel the filter applies to, or `None` for all channels.
    pub channel: Option<String>,
    /// The URL host pattern, or `None` for any host.
    pub host: Option<String>,
    /// The URL path pattern, or `None` for any path.
    pub path: Option<String>,
    /// The sender nickname pattern, or `None` for any nickname.
    pub nickname: Option<String>,
    /// The sender username (ident) pattern, or `None` for any username.
    pub username: Option<String>,
    /// The sender hostname pattern, or `None` for any hostname.
    pub hostname: Option<String>,
    /// The nickname of the user who created the filter.
    pub created_by: String,
    /// The time the filter was created.
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
impl Filter {
    /// Builds a filter from the fields of `new`, for tests that need one without a database.
    pub(crate) fn from_new_filter(id: i32, new: NewFilter) -> Self {
        Self {
            id,
            channel: new.channel,
            host: new.host,
            path: new.path,
            nickname: new.nickname,
            username: new.username,
            hostname: new.hostname,
            created_by: new.created_by,
            created_at: Utc::now(),
        }
    }
}

/// A filter to be inserted into the database.
#[derive(Debug, Clone)]
pub struct NewFilter {
    /// The channel the filter applies to, or `None` for all channels.
    pub channel: Option<String>,
    /// The URL host pattern, or `None` for any host.
    pub host: Option<String>,
    /// The URL path pattern, or `None` for any path.
    pub path: Option<String>,
    /// The sender nickname pattern, or `None` for any nickname.
    pub nickname: Option<String>,
    /// The sender username (ident) pattern, or `None` for any username.
    pub username: Option<String>,
    /// The sender hostname pattern, or `None` for any hostname.
    pub hostname: Option<String>,
    /// The nickname of the user creating the filter.
    pub created_by: String,
}
