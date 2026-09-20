//! Errors that can occur while handling notifications.

use crate::database::{DbError, database_error};

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

database_error!(Error, "notifications");
