use std::error::Error as StdError;

use thiserror::Error;

/// An error that occurred during plugin activity.
#[derive(Error, Debug)]
pub enum Error {
    #[error("IRC error: {0}")]
    Irc(#[from] irc::error::Error),
    #[error("Plugin error: {0}")]
    Plugin(Box<dyn StdError + Sync + Send>),
}
