//! Errors that can occur during IMDb interaction.

/// Errors that can occur during IMDb interaction.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not deserialize response: {0}")]
    Deserialize(#[source] serde_path_to_error::Error<serde_json::Error>),
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("graphql error: {0}")]
    GraphQL(String),
    #[error("resource not found")]
    NotFound,
    #[error("unexpected response from api")]
    UnexpectedResponse,
}
