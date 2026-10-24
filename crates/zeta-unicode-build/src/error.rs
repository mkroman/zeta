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
    /// An input path is not inside the data directory, so the manifest has no relative path
    /// for it.
    #[error(
        "input {} is outside the data directory {}; the manifest records only relative paths",
        .path.display(),
        .data_dir.display()
    )]
    OutsideDataDir {
        /// The input file that is not under the data directory.
        path: PathBuf,
        /// The data directory every input must live under.
        data_dir: PathBuf,
    },
}
