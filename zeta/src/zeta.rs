//! The main process for communicating over IRC and managing state.
use std::sync::Arc;

use futures::stream::StreamExt;
use irc::client::prelude::Client;
use irc::proto::Message;
use tracing::{debug, warn};

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
    ///
    /// Plugin errors are logged but not propagated — one failing plugin won't block others.
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
    /// If a plugin fails to handle a message, the error is logged but processing
    /// continues for remaining plugins. This prevents one misbehaving plugin from
    /// blocking all others.
    ///
    /// # Arguments
    /// * `client` - Reference to the IRC client for sending responses
    /// * `message` - The IRC message to process
    ///
    /// # Returns
    /// * `Ok(())` - Message processed (individual plugin errors are logged, not propagated)
    async fn handle_message(&self, client: &Client, message: Message) -> Result<(), Error> {
        debug!(?message, "processing irc message");

        for (plugin_name, plugin) in &self.registry.plugins {
            if let Err(e) = plugin.handle_message(&self.context, client, &message).await {
                warn!(plugin = %plugin_name, error = %e, "plugin error during message handling");
            }
        }

        Ok(())
    }
}
