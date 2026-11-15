//! Database connection configuration.
//!
//! [`DbConfig`] holds the connection URL and the pool bounds that the host applies to its
//! connection pool. The URL usually embeds a password, so the manual `Debug` implementation
//! renders it through the shared URL redaction instead of exposing it in logs.

use std::{fmt, time::Duration};

use serde::Deserialize;
use url::Url;
use zeta_url::RedactedUrl;

use crate::consts;

/// Database connection configuration.
#[derive(Clone, Deserialize)]
pub struct DbConfig {
    /// Connection URL
    pub url: String,
    /// Minimum number of connections to keep active in the connection pool
    #[serde(default = "default_min_db_connections")]
    pub min_connections: u32,
    /// Maximum number of connections to keep active in the connection pool
    #[serde(default = "default_max_db_connections")]
    pub max_connections: u32,
    /// Maximum idle duration for individual connections.
    #[serde(default = "default_db_idle_timeout", with = "humantime_serde")]
    pub idle_timeout: Duration,
    /// Set the maximum lifetime of individual connections.
    ///
    /// Any connection with a lifetime greater than this will be closed.
    #[serde(default = "default_db_max_lifetime", with = "humantime_serde")]
    pub max_lifetime: Duration,
}

// Manual implementation so the password embedded in the connection URL is never printed.
impl fmt::Debug for DbConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbConfig")
            .field("url", &RedactedDbUrl(&self.url))
            .field("min_connections", &self.min_connections)
            .field("max_connections", &self.max_connections)
            .field("idle_timeout", &self.idle_timeout)
            .field("max_lifetime", &self.max_lifetime)
            .finish()
    }
}

/// Renders a connection URL through the shared URL redaction.
///
/// A connection string that is not a URL (libpq's `host=... password=...` form) cannot be
/// redacted structurally, so it is hidden entirely rather than risking a leaked password.
struct RedactedDbUrl<'a>(&'a str);

impl fmt::Debug for RedactedDbUrl<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match Url::parse(self.0) {
            Ok(url) => fmt::Debug::fmt(&RedactedUrl::from_url(&url), f),
            Err(_) => f.write_str("<redacted>"),
        }
    }
}

/// Returns the default value for number of minimum database connections.
const fn default_min_db_connections() -> u32 {
    consts::DEFAULT_MIN_DB_CONNECTIONS
}

/// Returns the default value for number of maximum database connections.
const fn default_max_db_connections() -> u32 {
    consts::DEFAULT_MAX_DB_CONNECTIONS
}

/// Returns the default duration a connection can be idle before it is dropped.
const fn default_db_idle_timeout() -> Duration {
    consts::DEFAULT_DB_IDLE_TIMEOUT
}

/// Returns the default maximum lifetime of database pool connections.
const fn default_db_max_lifetime() -> Duration {
    consts::DEFAULT_DB_MAX_LIFETIME
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_url_uses_the_shared_redaction() {
        assert_eq!(
            format!(
                "{:?}",
                RedactedDbUrl("postgresql://zeta:secret@localhost/zeta_test")
            ),
            r#"RedactedUrl("postgresql://localhost/zeta_test")"#
        );
        // The URL redaction drops userinfo, query and fragment. A connection string that is not
        // a URL cannot be redacted structurally and is hidden instead.
        assert_eq!(
            format!("{:?}", RedactedDbUrl("host=localhost password=secret")),
            "<redacted>"
        );
    }
}
