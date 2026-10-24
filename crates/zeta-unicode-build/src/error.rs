//! The I/O errors the trainer and manifest writer report.

use std::path::PathBuf;

use thiserror::Error;

/// Fails a training run when an input cannot be read or an artifact cannot be written.
#[derive(Debug, Error)]
pub enum Error {
    /// Reading a vendored UCD input failed.
    #[error("failed to read {}: {source}", .path.display())]
    Read {
        /// The file that could not be read.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// Writing a generated artifact failed.
    #[error("failed to write {}: {source}", .path.display())]
    Write {
        /// The file that could not be written.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}
