//! HTTP features
//!
//! Two independent clients live here: the plain `reqwest` client shared by the API plugins
//! (behind the `http` feature) and the browser-emulated `wreq` client for anti-bot-protected
//! sites (behind the `emulated` feature).

#[cfg(feature = "http")]
use crate::config::HttpConfig;
#[cfg(feature = "http")]
use serde::de::DeserializeOwned;

/// JSON response parsing shared by the API client plugins.
///
/// Bodies are parsed with [`serde_path_to_error`] so parse failures report the path to the
/// offending part of the document, and the offending body is logged.
#[cfg(feature = "http")]
pub mod json {
    use serde::de::DeserializeOwned;
    use tracing::error;

    /// The error returned when a JSON body fails to parse.
    pub type Error = serde_path_to_error::Error<serde_json::Error>;

    /// Deserializes a value of type `T` from a JSON string.
    ///
    /// The offending body is logged if parsing fails.
    ///
    /// # Errors
    ///
    /// Returns the parse error, including the path to the offending part of the document, if
    /// `text` is not valid JSON for `T`.
    pub fn from_str<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
        let deserializer = &mut serde_json::Deserializer::from_str(text);

        serde_path_to_error::deserialize(deserializer).inspect_err(|error| {
            error!(?error, body = %text, "could not deserialize json response");
        })
    }
}

/// The errors produced when sending a request and parsing its JSON response.
#[cfg(feature = "http")]
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The request could not be sent, or the response body could not be read.
    #[error("request error: {0}")]
    Request(reqwest::Error),
    /// The server responded with a non-success status code, e.g. `404 Not Found`.
    #[error("{status}")]
    Status {
        /// The response status.
        status: reqwest::StatusCode,
        /// The error response body, read so callers can diagnose the failure (e.g. an API
        /// error message inside it).
        body: String,
    },
    /// The response body could not be parsed as JSON.
    #[error("could not deserialize response: {0}")]
    Deserialize(json::Error),
}

/// Sends a request built by [`reqwest::Client::get`] (or a sibling builder method) and parses
/// its JSON body into `T`.
///
/// Non-success statuses are reported as [`ApiError::Status`], carrying the response body; the
/// response is only parsed as JSON when the status is a success.
///
/// # Errors
///
/// Returns an [`ApiError`] if the request fails, the response status is not a success, or the
/// body cannot be parsed as JSON.
#[cfg(feature = "http")]
pub async fn parse_response<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ApiError> {
    let status = response.status();

    if !status.is_success() {
        let body = response.text().await.map_err(ApiError::Request)?;

        return Err(ApiError::Status { status, body });
    }

    let text = response.text().await.map_err(ApiError::Request)?;

    json::from_str(&text).map_err(ApiError::Deserialize)
}

/// Parses a JSON response like [`parse_response`], mapping a `404` status to `not_found`.
///
/// The remaining [`ApiError`]s are converted into `E`.
///
/// # Errors
///
/// Returns `not_found` if the response status is `404 Not Found`; any other [`parse_response`]
/// error is converted into `E`.
#[cfg(feature = "http")]
pub async fn parse_response_or_404<T: DeserializeOwned, E: From<ApiError>>(
    response: reqwest::Response,
    not_found: E,
) -> Result<T, E> {
    match parse_response(response).await {
        Ok(value) => Ok(value),
        Err(ApiError::Status {
            status: reqwest::StatusCode::NOT_FOUND,
            ..
        }) => Err(not_found),
        Err(error) => Err(error.into()),
    }
}

/// HTTP client integration
#[cfg(feature = "http")]
pub mod client {
    use crate::config::HttpConfig;

    pub use reqwest::Client;
    use reqwest::redirect::Policy;

    /// Returns a default HTTP client configured by [`HttpConfig`].
    ///
    /// # Panics
    ///
    /// Panics if the default HTTP client fails to build.
    #[must_use]
    pub fn build(config: &HttpConfig) -> Client {
        builder(config)
            .build()
            .expect("could not build http client")
    }

    /// Returns a default HTTP client builder configured by [`HttpConfig`].
    pub fn builder(config: &HttpConfig) -> reqwest::ClientBuilder {
        reqwest::ClientBuilder::new()
            .redirect(Policy::none())
            .timeout(config.timeout)
            .user_agent(config.user_agent.clone())
    }
}

/// Builds a default HTTP client configured by [`HttpConfig`].
///
/// This is equivalent to calling [`client::build`].
#[must_use]
#[cfg(feature = "http")]
pub fn build_client(config: &HttpConfig) -> client::Client {
    client::build(config)
}

/// Client building for anti-bot-protected sites.
///
/// These are fetched with [`wreq`] browser emulation: plain `reqwest` gets TLS-fingerprinted by
/// them regardless of headers. Built behind the `emulated` feature, which the anti-bot-protected
/// plugins enable.
#[cfg(feature = "emulated")]
pub mod emulated {
    use wreq::header::{HeaderMap, HeaderValue, ACCEPT_ENCODING, USER_AGENT};
    use zeta_plugin::{Error, prelude::plugin_err};

    /// Returns a `wreq` client builder emulating Firefox 142, layered with the headers the
    /// emulation must not omit.
    ///
    /// `Accept-Encoding` must be set explicitly: like reqwest, wreq does not advertise the
    /// header itself even though it decompresses responses — and its absence is enough to get
    /// flagged.
    ///
    /// The profile's user agent is overridden with `user_agent` when given. This skews with the
    /// emulated Firefox 142 fingerprint, and that is deliberate: the profile defaults to macOS —
    /// which anti-bot systems score far more aggressively when requests originate from
    /// datacenter networks — and the matching Linux Firefox 142 user agent is rejected by
    /// DataDome outright, while Firefox 151 passes.
    ///
    /// # Errors
    ///
    /// Returns an error if `user_agent` cannot be used as a header value.
    pub fn builder(user_agent: Option<&str>) -> Result<wreq::ClientBuilder, Error> {
        let mut headers = HeaderMap::new();

        headers.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br, zstd"),
        );

        if let Some(user_agent) = user_agent {
            let value = HeaderValue::from_str(user_agent).map_err(plugin_err)?;

            headers.insert(USER_AGENT, value);
        }

        Ok(wreq::Client::builder().emulation(wreq_util::Emulation::Firefox142).default_headers(headers))
    }
}
