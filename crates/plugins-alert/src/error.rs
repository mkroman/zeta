//! Errors that can occur while handling alerts.

use zeta_plugin::database::DbError;
use zeta_plugin::impl_repository_error;

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

impl_repository_error!(Error, "alerts");
