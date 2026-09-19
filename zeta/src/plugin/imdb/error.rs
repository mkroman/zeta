//! Errors that can occur during IMDb interaction.

/// Errors that can occur during IMDb interaction.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The API response could not be deserialized.
    #[error("could not deserialize response: {0}")]
    Deserialize(#[from] serde_path_to_error::Error<serde_json::Error>),
    /// Sending the HTTP request failed.
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    /// A configured request header is not a valid header value.
    #[error("invalid header value: {0}")]
    InvalidHeader(#[from] reqwest::header::InvalidHeaderValue),
    /// The GraphQL API returned an error in its response.
    #[error("graphql error: {0}")]
    GraphQL(String),
    /// The requested title or person does not exist.
    #[error("resource not found")]
    NotFound,
    /// The API returned a response the client does not handle.
    #[error("unexpected response from api")]
    UnexpectedResponse,
}
