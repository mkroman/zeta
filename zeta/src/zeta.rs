//! The main process for communicating over IRC and managing state.
use futures::stream::StreamExt;
use irc::client::prelude::Client;
use irc::proto::Message;
use tracing::debug;

use crate::Error;
use crate::Registry;
use crate::config::Config;

/// The main IRC bot struct that manages connection state and message handling.
pub struct Zeta {
    /// The complete configuration loaded from file or environment
    config: Config,
    /// The IRC client - None until connection is established
    client: Option<Client>,
    /// The plugin containing all loaded plugins
    registry: Registry,
}

impl Zeta {
    /// Creates a new Zeta instance from the provided configuration.
    ///
    /// This initializes the plugin registry with preloaded plugins but doesn't
    /// establish the IRC connection yet. Call `run()` to start the bot.
    ///
    /// # Arguments
    /// * `config` - The bot configuration containing IRC server details and settings
    ///
    /// # Returns
    /// * `Ok(Zeta)` - Successfully created bot instance
    /// * `Err(Error)` - If plugin registry initialization fails
    pub fn from_config(config: Config) -> Result<Self, Error> {
        let registry = Registry::new();

        Ok(Zeta {
            client: None,
            registry,
            config,
        })
    }

    /// Starts the bot and begins processing IRC messages.
    pub async fn run(&mut self) -> Result<(), Error> {
        let mut client = Client::from_config(self.config.irc.clone().into())
            .await
            .map_err(Error::IrcClientError)?;

        client.identify().map_err(Error::IrcRegistrationError)?;

        let mut stream = client.stream()?;

        self.client = Some(client);

        if let Some(client) = &self.client {
            while let Some(message) = stream.next().await.transpose()? {
                self.handle_message(client, message).await?;
            }
        }

        Ok(())
    }

    /// Processes a single IRC message by dispatching it to all registered plugins.
    ///
    /// This method logs the incoming message for debugging and then forwards it
    /// to each plugin in the registry for processing. Plugins can respond to
    /// messages, update state, or perform other actions as needed.
    ///
    /// # Arguments
    /// * `client` - Reference to the IRC client for sending responses
    /// * `message` - The IRC message to process
    ///
    /// # Returns
    /// * `Ok(())` - Message processed successfully by all plugins
    /// * `Err(Error)` - One or more plugins failed to process the message
    async fn handle_message(&self, client: &Client, message: Message) -> Result<(), Error> {
        debug!(?message, "processing irc message");

        for plugin in &self.registry.plugins {
            plugin.handle_message(&message, client).await?;
        }

        Ok(())
    }

    pub async fn load_plugins(&mut self) -> Result<(), Error> {
        let plugin_configs = &self.config.plugins;

        self.registry.load_plugins(plugin_configs).await?;

        Ok(())
    }
}
