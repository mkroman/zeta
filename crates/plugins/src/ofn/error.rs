//! Errors that can occur while recording or looking up URLs.

/// Errors that can occur while recording or looking up URLs.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A read from the database failed.
    #[error("could not query database: {0}")]
    QueryDatabase(#[source] sqlx::Error),
    /// A URL could not be inserted into the database.
    #[error("could not insert url in database: {0}")]
    InsertUrl(#[source] sqlx::Error),
    /// The URL has no host and cannot be recorded.
    #[error("can't insert url with no host")]
    InsertUrlNoHost,
}
