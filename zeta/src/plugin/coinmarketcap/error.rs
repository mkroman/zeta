//! Errors that can occur during coin lookups.

/// Errors that can occur during coin lookups.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The HTTP request failed.
    #[error("request error")]
    Request(#[source] reqwest::Error),
    /// The response could not be parsed.
    #[error("could not parse response")]
    Deserialize(#[source] serde_path_to_error::Error<serde_json::Error>),
    /// The API returned an error status (e.g. an invalid symbol).
    #[error("{0}")]
    Api(String),
    /// An unsupported conversion currency was requested.
    #[error("Invalid currency: {0}")]
    InvalidCurrency(String),
    /// The requested coin was not found.
    #[error("not found")]
    NotFound,
}
