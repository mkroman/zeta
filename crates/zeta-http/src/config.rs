//! HTTP client configuration.
//!
//! The host deserializes this section and hands it to the plugins through their context; the
//! shared client builders in this crate consume it directly.

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

#[cfg(feature = "http")]
impl HttpConfig {
    /// Validates the values the shared HTTP client is built from.
    ///
    /// The client builder cannot represent an invalid `user_agent`, and failing at configuration
    /// load is preferable to panicking on the first request — the shared client is built lazily.
    ///
    /// # Errors
    ///
    /// Returns an error when `user_agent` is not a valid HTTP header value.
    pub fn validate(&self) -> Result<(), reqwest::header::InvalidHeaderValue> {
        reqwest::header::HeaderValue::try_from(self.user_agent.as_str()).map(|_| ())
    }
}
