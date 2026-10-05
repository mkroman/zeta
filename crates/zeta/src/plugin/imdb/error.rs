//! Errors that can occur during IMDb interaction.

use crate::{error::RequestError, http};

/// Errors that can occur during IMDb interaction.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The API request failed or its response could not be handled.
    #[error(transparent)]
    Api(#[from] http::ApiError),
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

impl From<RequestError> for Error {
    fn from(error: RequestError) -> Self {
        Self::Api(error.into())
    }
}
