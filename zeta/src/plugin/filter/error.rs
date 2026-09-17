//! Errors that can occur while handling filters.

use crate::database::DbError;

/// Errors that can occur while handling filters.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A database operation on the filters failed.
    #[error(transparent)]
    Database(#[from] DbError),
}

impl Error {
    /// Constructs the error for a failed load of filters.
    pub(crate) const fn load(source: sqlx::Error) -> Self {
        Self::Database(DbError::load("filters", source))
    }

    /// Constructs the error for a failed insert of a filter.
    pub(crate) const fn insert(source: sqlx::Error) -> Self {
        Self::Database(DbError::insert("filters", source))
    }

    /// Constructs the error for a failed delete of filters.
    pub(crate) const fn delete(source: sqlx::Error) -> Self {
        Self::Database(DbError::delete("filters", source))
    }
}
