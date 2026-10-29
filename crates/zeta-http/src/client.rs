//! The plain `reqwest` client shared by the API plugins.
//!
//! [`send`] logs a failed request once before the error comes back as a [`RequestError`], and
//! [`builder`]/[`build_client`] build the client from [`HttpConfig`]. Reading a body — and
//! logging a failed read — lives in [`crate::response`], so JSON and text responses share one
//! status-handling path.

use tracing::error;
use zeta_url::RedactedUrl;

#[cfg(feature = "http")]
use crate::HttpConfig;
use crate::RequestError;

/// Sends `request` and returns the response.
///
/// A failed send is logged once with `url.full` — the request URL without its query, fragment,
/// or userinfo — its
/// `error.type`, and the error's full rendering, before the error comes back as a
/// [`RequestError`]: these APIs carry their key in the query string, and the error reaches
/// `Display` (and `Debug`) in plugin and dispatcher logs, while a log line still needs to say
/// which request failed and why.
///
/// # Errors
///
/// Returns the [`RequestError`] of the failed send, with its URL redacted.
pub async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response, RequestError> {
    request.send().await.map_err(|error| {
        let error = RequestError::from(error);

        if let Some(url) = error.url() {
            log_request_error(&error, url, "request failed");
        }

        error
    })
}

/// Logs a failed request once, with its redacted URL and error type.
fn log_request_error(error: &RequestError, url: &RedactedUrl, message: &str) {
    error!(url.full = %url, error.type = error.error_type(), error = %error.full(), "{message}");
}

/// Returns a default HTTP client configured by [`HttpConfig`].
///
/// # Panics
///
/// Panics if the default HTTP client fails to build.
#[must_use]
#[cfg(feature = "http")]
pub fn build_client(config: &HttpConfig) -> reqwest::Client {
    builder(config)
        .build()
        .expect("could not build http client")
}

/// Returns a default HTTP client builder configured by [`HttpConfig`].
#[cfg(feature = "http")]
pub fn builder(config: &HttpConfig) -> reqwest::ClientBuilder {
    reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(config.timeout)
        .user_agent(config.user_agent.clone())
}
