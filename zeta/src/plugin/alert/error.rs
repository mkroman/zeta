//! Errors that can occur while handling alerts.

/// Errors that can occur while handling alerts.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Loading alerts from the database failed.
    #[error("could not load alerts from database")]
    Load(#[source] sqlx::Error),
    /// Inserting an alert into the database failed.
    #[error("could not insert alert: {0}")]
    Insert(#[source] sqlx::Error),
    /// Deleting an alert from the database failed.
    #[error("could not delete alert: {0}")]
    Delete(#[source] sqlx::Error),
    /// The alert delivery channel is closed.
    #[error("alert delivery channel is closed")]
    Closed,
}
