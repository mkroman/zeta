//! Errors that can occur while unwalling.

use crate::database::{DbError, database_error};
use crate::error::RequestError;
use crate::http::ApiError;

/// Errors that can occur while unwalling.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A database operation on the unwall state failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// The request to the unwall API could not be sent.
    #[error(transparent)]
    Request(#[from] RequestError),
    /// The unwall API response could not be parsed.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// The unwall API rejected the article with an error status.
    #[error("unwall could not fetch the article (HTTP {0})")]
    Status(u16),
    /// The article URL is not usable.
    #[error("not a usable article URL")]
    InvalidUrl,
    /// A configured base URL could not be parsed.
    #[error("invalid base URL: {0}")]
    BaseUrl(#[from] url::ParseError),
}

database_error!(Error, "unwall state");
