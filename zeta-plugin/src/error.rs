use std::error::Error as StdError;
use std::fmt::Display;

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
///
/// # Example
///
/// ```ignore
/// fn new(_ctx: &Context) -> Result<Self, ZetaError> {
///     let api_key = env::var("API_KEY").map_err(plugin_err)?;
///     Ok(Self { api_key })
/// }
/// ```
pub fn plugin_err<E: StdError + Send + Sync + 'static>(e: E) -> Error {
    Error::Plugin(Box::new(e))
}

/// Wraps any displayable error value into [`Error::Plugin`].
///
/// For types that only implement [`Display`] but not [`StdError`],
/// this converts the error message into an [`std::io::Error`] before boxing.
pub fn plugin_err_display<E: Display + Send + Sync + 'static>(e: E) -> Error {
    Error::Plugin(Box::new(std::io::Error::other(e.to_string())))
}
