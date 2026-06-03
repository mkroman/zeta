use std::error::Error as StdError;

use thiserror::Error;

/// A boxed, trait-object error for plugin initialization.
///
/// Used as the return type for `Plugin::new()` to allow each plugin
/// to return heterogeneous errors without coupling to a specific enum.
pub type BoxError = Box<dyn StdError + Send + Sync>;

/// An error that occurred during plugin activity.
#[derive(Error, Debug)]
pub enum Error {
    #[error("IRC error: {0}")]
    Irc(#[from] irc::error::Error),
    #[error("Plugin error: {0}")]
    Plugin(BoxError),
}
