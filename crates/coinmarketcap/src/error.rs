//! Error types for the CoinMarketCap client.

use zeta_http::RequestError;

/// The JSON response body could not be parsed.
pub type ParseError = serde_path_to_error::Error<serde_json::Error>;

/// Errors that can occur during coin lookups.
///
/// The [`Display`](std::fmt::Display) representation of each variant is the message that is
/// replied to the channel.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The client could not be built.
    #[error("Could not reach the CoinMarketCap API")]
    Build(#[source] RequestError),
    /// The configured API key is not a valid HTTP header value.
    #[error("Invalid CoinMarketCap API key")]
    InvalidApiKey(#[source] reqwest::header::InvalidHeaderValue),
    /// The HTTP request failed.
    #[error("Could not reach the CoinMarketCap API")]
    Request(#[from] RequestError),
    /// The response could not be parsed.
    #[error("Could not parse the response from the CoinMarketCap API")]
    Deserialize(#[source] ParseError),
    /// The API returned an error status (e.g. an invalid symbol).
    #[error("Could not retrieve coin information: {0}")]
    Api(String),
    /// An unsupported conversion currency was requested.
    #[error("Unsupported currency: {0}")]
    InvalidCurrency(String),
    /// The requested coin was not found.
    #[error("No such coin")]
    NotFound,
}
