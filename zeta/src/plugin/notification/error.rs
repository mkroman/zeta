//! Errors that can occur while handling notifications.

use crate::database::DbError;

/// Errors that can occur while handling notifications.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A database operation on the notifications failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// The target already has the maximum number of pending notifications.
    #[error("too many pending notifications (maximum is {0})")]
    TooManyPending(usize),
}

impl Error {
    /// Constructs the error for a failed load of notifications.
    pub(crate) const fn load(source: sqlx::Error) -> Self {
        Self::Database(DbError::load("notifications", source))
    }

    /// Constructs the error for a failed insert of a notification.
    pub(crate) const fn insert(source: sqlx::Error) -> Self {
        Self::Database(DbError::insert("notifications", source))
    }

    /// Constructs the error for a failed delete of notifications.
    pub(crate) const fn delete(source: sqlx::Error) -> Self {
        Self::Database(DbError::delete("notifications", source))
    }
}
