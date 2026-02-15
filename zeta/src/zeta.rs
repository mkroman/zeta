//! The main process for communicating over IRC and managing state.
use std::sync::Arc;

use futures::stream::StreamExt;
use irc::client::prelude::Client;
use irc::proto::Message;
use tracing::debug;

use crate::Error;
use crate::Registry;
use crate::config::Config;
use crate::plugin::Context;

/// The main IRC bot struct that manages connection state and message handling.
pub struct Zeta {
    /// The complete configuration loaded from file or environment
    config: Config,
    /// The IRC client - None until connection is established
    client: Option<Client>,
    /// The plugin containing all loaded plugins
    registry: Registry,
    /// The shared context for plugins
    context: Arc<Context>,
}

impl Zeta {
    /// Creates a new Zeta instance from the provided configuration.
    ///
    /// This initializes the plugin registry with preloaded plugins but doesn't
    /// establish the IRC connection yet. Call `run()` to start the bot.
    #[must_use]
    pub fn new(
        config: Config,
        #[cfg(feature = "database")] db: crate::database::Database,
        dns: hickory_resolver::TokioResolver,
    ) -> Self {
        let context = Arc::new(Context::new(
            #[cfg(feature = "database")]
            db,
            dns,
            config.clone(),
        ));
        let registry = Registry::preloaded(&context);

        Zeta {
            client: None,
            registry,
            config,
            context,
        }
    }

    /// Starts the bot and begins processing IRC messages.
    ///
    /// # Errors
    ///
    /// This function will return an error in the following situations:
    ///
    /// - [`Error::IrcClient`] - if the instantiation of the IRC client fails (e.g. due to
    ///   configuration issues.)
    /// - [`Error::IrcRegistration`] - if user registration fails (e.g. if the nickname is already taken.)
    /// - [`Error::Irc`] - if a protocol or communication error occurred.
    /// - [`Error::Plugin`] - if a plugins [`handle_message`] function returns an error
    ///
    /// [`handle_message`]: crate::plugin::Plugin::handle_message
    pub async fn run(&mut self) -> Result<(), Error> {
        let mut client = Client::from_config(self.config.irc.clone().into())
            .await
            .map_err(Error::IrcClient)?;

        client.identify().map_err(Error::IrcRegistration)?;

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
            plugin
                .handle_message(&self.context, client, &message)
                .await?;
        }

        Ok(())
    }
}
