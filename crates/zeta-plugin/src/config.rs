//! Plugin-facing configuration.
//!
//! The host keeps the full configuration; plugins only see the
//! [`HostConfig`] subset carried on their [`Context`](crate::Context), plus the generic
//! [`HttpConfig`] section the shared HTTP clients are built from.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The `User-Agent` header to send when issuing HTTP requests.
const HTTP_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64; rv:157.0) Gecko/20100101 Firefox/157.0";

/// The duration before a HTTP request times out.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP client configuration.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct HttpConfig {
    /// Duration before an HTTP request times out.
    #[serde(with = "humantime_serde")]
    pub timeout: Duration,
    /// The `User-Agent` header sent with HTTP requests.
    pub user_agent: String,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout: HTTP_TIMEOUT,
            user_agent: HTTP_USER_AGENT.to_string(),
        }
    }
}

/// The subset of the host configuration plugins can see through their context.
///
/// This is deliberately narrower than the host's configuration: per-plugin settings are handed
/// to each plugin through its constructor, and host-only sections (database, tracing, the IRC
/// connection, the media mirror) never reach plugin code.
#[derive(Clone, Debug, Default)]
pub struct HostConfig {
    /// HTTP client configuration, for plugins that build their own clients.
    pub http: HttpConfig,
    /// Hostmasks of admin users, in `nick!user@hostname` form, where any of the three
    /// components may contain wildcards (`*` or `?`), e.g. `mk!mk@*` or `*!*@example.com`.
    ///
    /// Mirrored from the host's `[irc].admin_hostmasks` for plugins that authorize
    /// administrative commands.
    pub admin_hostmasks: Vec<String>,
}
