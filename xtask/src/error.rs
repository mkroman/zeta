//! A single error type for task failures.

use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

/// A task failed with a message suitable for `eprintln!`.
#[derive(Debug)]
pub struct Error(String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<crate::json::Error> for Error {
    fn from(error: crate::json::Error) -> Self {
        Self(error.to_string())
    }
}
