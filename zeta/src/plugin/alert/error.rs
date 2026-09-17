//! Errors that can occur while handling alerts.

use crate::database::DbError;

/// Errors that can occur while handling alerts.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A database operation on the alerts failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// The alert delivery channel is closed.
    #[error("alert delivery channel is closed")]
    Closed,
}

impl Error {
    /// Constructs the error for a failed load of alerts.
    pub(crate) const fn load(source: sqlx::Error) -> Self {
        Self::Database(DbError::load("alerts", source))
    }

    /// Constructs the error for a failed insert of an alert.
    pub(crate) const fn insert(source: sqlx::Error) -> Self {
        Self::Database(DbError::insert("alerts", source))
    }

    /// Constructs the error for a failed delete of alerts.
    pub(crate) const fn delete(source: sqlx::Error) -> Self {
        Self::Database(DbError::delete("alerts", source))
    }
}
