//! The plain `reqwest` client shared by the API plugins.
//!
//! [`send`] logs a failed request once before the error comes back as a [`RequestError`],
//! [`text`] does the same for a failed body read, and [`builder`]/[`build_client`] build the
//! client from [`HttpConfig`].

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

/// Reads the body of `response` as a string.
///
/// A failed read is logged once with `url.full` — the response URL without its query, fragment,
/// or userinfo — the
/// way [`send`] logs a failed request, so no caller has to hand-roll that log line.
///
/// # Errors
///
/// Returns the [`RequestError`] of the failed read, with its URL redacted.
#[cfg(feature = "http")]
pub async fn text(response: reqwest::Response) -> Result<String, RequestError> {
    let url = RedactedUrl::from_url(response.url());

    response.text().await.map_err(|error| {
        let error = RequestError::from(error);

        log_request_error(&error, &url, "reading response body failed");

        error
    })
}

/// Logs a failed request or body read once, with its redacted URL and error type.
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
