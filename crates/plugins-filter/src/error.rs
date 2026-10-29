//! Errors that can occur while handling filters.

use zeta_plugin::database::DbError;
use zeta_plugin::impl_repository_error;

/// Errors that can occur while handling filters.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A database operation on the filters failed.
    #[error(transparent)]
    Database(#[from] DbError),
}

impl_repository_error!(Error, "filters");
