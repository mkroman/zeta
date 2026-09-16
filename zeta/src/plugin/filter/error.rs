//! Errors that can occur while handling filters.

/// Errors that can occur while handling filters.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Loading filters from the database failed.
    #[error("could not load filters from database")]
    Load(#[source] sqlx::Error),
    /// Inserting a filter into the database failed.
    #[error("could not insert filter: {0}")]
    Insert(#[source] sqlx::Error),
    /// Deleting filters from the database failed.
    #[error("could not delete filters: {0}")]
    Delete(#[source] sqlx::Error),
}
