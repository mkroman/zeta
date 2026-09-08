//! Errors that can occur while handling alerts.

/// Errors that can occur while handling alerts.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Loading alerts from the database failed.
    #[error("could not load alerts from database")]
    Load(#[source] sqlx::Error),
    /// Inserting an alert into the database failed.
    #[error("could not insert alert: {0}")]
    Insert(#[source] sqlx::Error),
    /// Deleting an alert from the database failed.
    #[error("could not delete alert: {0}")]
    Delete(#[source] sqlx::Error),
}

/// Errors that can occur while parsing the datetime of an alert.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseTimeError {
    /// The datetime could not be understood.
    #[error("ambiguous or unsupported datetime")]
    Ambiguous,
    /// The datetime occurs in the past.
    #[error("specified time occurs in the past")]
    Past,
}
