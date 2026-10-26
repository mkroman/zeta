//! Errors that can occur while recording or looking up URLs.

use zeta_plugin::database::DbError;
use zeta_plugin::database_error;

/// Errors that can occur while recording or looking up URLs.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A database operation on the OFN records failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// The URL has no host and cannot be recorded.
    #[error("can't insert url with no host")]
    InsertUrlNoHost,
}

database_error!(Error, "ofn records");
