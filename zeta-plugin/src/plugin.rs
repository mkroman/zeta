use async_trait::async_trait;
use irc::client::Client;
use irc::proto::Message;

use crate::{Author, Error, Name, Version};

/// The base trait that all plugins must implement.
#[async_trait]
pub trait Plugin: Send + Sync {
    /// The constructor for a new plugin.
    fn new() -> Self
    where
        Self: Sized;

    /// Returns the name of the plugin.
    fn name() -> Name
    where
        Self: Sized;

    /// Returns the author of the plugin.
    fn author() -> Author
    where
        Self: Sized;

    /// Returns the version of the plugin.
    fn version() -> Version
    where
        Self: Sized;

    /// Handles IRC protocol messages.
    async fn handle_message(&self, _message: &Message, _client: &Client) -> Result<(), Error> {
        Ok(())
    }
}
