//! Errors that can occur while handling filters.

use crate::database::{DbError, database_error};

/// Errors that can occur while handling filters.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A database operation on the filters failed.
    #[error(transparent)]
    Database(#[from] DbError),
}

database_error!(Error, "filters");
