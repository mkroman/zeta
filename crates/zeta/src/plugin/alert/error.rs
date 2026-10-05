//! Errors that can occur while handling alerts.

use crate::database::{DbError, database_error};

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

database_error!(Error, "alerts");
