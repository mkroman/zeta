//! Errors that can occur while handling notifications.

use zeta_plugin::database::DbError;
use zeta_plugin::impl_repository_error;

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

impl_repository_error!(Error, "notifications");
