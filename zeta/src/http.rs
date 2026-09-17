//! HTTP features

use crate::config::HttpConfig;
use serde::de::DeserializeOwned;

/// JSON response parsing shared by the API client plugins.
///
/// Bodies are parsed with [`serde_path_to_error`] so parse failures report the path to the
/// offending part of the document, and the offending body is logged.
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
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// The request could not be sent, or the response body could not be read.
    #[error("request error: {0}")]
    Request(reqwest::Error),
    /// The server responded with a non-success status code, e.g. `404 Not Found`.
    #[error("{0}")]
    Status(reqwest::StatusCode),
    /// The response body could not be parsed as JSON.
    #[error("could not deserialize response: {0}")]
    Deserialize(json::Error),
}

/// Sends a request built by [`reqwest::Client::get`] (or a sibling builder method) and parses
/// its JSON body into `T`.
///
/// Non-success statuses are reported as [`ApiError::Status`] without reading the body; the
/// response is only parsed when the status is a success.
///
/// # Errors
///
/// Returns an [`ApiError`] if the request fails, the response status is not a success, or the
/// body cannot be parsed as JSON.
pub async fn parse_response<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, ApiError> {
    let status = response.status();

    if !status.is_success() {
        return Err(ApiError::Status(status));
    }

    let text = response.text().await.map_err(ApiError::Request)?;

    json::from_str(&text).map_err(ApiError::Deserialize)
}

/// HTTP client integration
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
    #[allow(unused)]
    pub fn build(config: &HttpConfig) -> Client {
        builder(config)
            .build()
            .expect("could not build http client")
    }

    /// Returns a default HTTP client builder configured by [`HttpConfig`].
    #[allow(unused)]
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
#[allow(unused)]
pub fn build_client(config: &HttpConfig) -> client::Client {
    client::build(config)
}
