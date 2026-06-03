use std::error::Error as StdError;

use thiserror::Error;

/// A boxed, trait-object error for plugin initialization.
///
/// Used internally in plugin `new()` methods to allow heterogeneous errors
/// before wrapping into [`Error::Plugin`].
pub type BoxError = Box<dyn StdError + Send + Sync>;

/// An error that occurred during plugin activity.
#[derive(Error, Debug)]
pub enum Error {
    #[error("IRC error: {0}")]
    Irc(#[from] irc::error::Error),
    #[error("Plugin error: {0}")]
    Plugin(BoxError),
}

impl From<BoxError> for Error {
    fn from(e: BoxError) -> Self {
        Self::Plugin(e)
    }
}

/// Wraps any error into [`Error::Plugin`].
///
/// Accepts errors that implement [`StdError + Send + Sync`] and boxes them
/// into a [`BoxError`], then wraps into [`Error::Plugin`].
pub fn plugin_err<E: StdError + Send + Sync + 'static>(e: E) -> Error {
    Error::Plugin(Box::new(e))
}

/// Reads a required environment variable, returning a descriptive error on failure.
///
/// # Errors
///
/// Returns [`Error::Plugin`] if the variable is not set or contains invalid
/// UTF-8. The error message includes the variable name.
///
/// # Example
///
/// ```ignore
/// fn new(_ctx: &Context) -> Result<Self, ZetaError> {
///     let api_key = require_env("API_KEY")?;
///     Ok(Self { api_key })
/// }
/// ```
pub fn require_env(name: &str) -> Result<String, Error> {
    std::env::var(name).map_err(|e| {
        Error::Plugin(Box::new(std::io::Error::other(format!(
            "environment variable `{name}`: {e}"
        ))))
    })
}
