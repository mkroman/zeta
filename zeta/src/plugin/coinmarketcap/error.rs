//! Errors that can occur during coin lookups.

/// Errors that can occur during coin lookups.
///
/// The [`Display`](std::fmt::Display) representation of each variant is the message that is
/// replied to the channel.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The HTTP request failed.
    #[error("Could not reach the CoinMarketCap API")]
    Request(#[source] reqwest::Error),
    /// The response could not be parsed.
    #[error("Could not parse the response from the CoinMarketCap API")]
    Deserialize(#[source] serde_path_to_error::Error<serde_json::Error>),
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
