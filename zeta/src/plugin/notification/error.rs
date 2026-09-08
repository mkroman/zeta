//! Errors that can occur while handling notifications.

/// Errors that can occur while handling notifications.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Loading notifications from the database failed.
    #[error("could not load notifications from database")]
    Load(#[source] sqlx::Error),
    /// Inserting a notification into the database failed.
    #[error("could not insert notification: {0}")]
    Insert(#[source] sqlx::Error),
    /// Deleting a notification from the database failed.
    #[error("could not delete notification: {0}")]
    Delete(#[source] sqlx::Error),
}
