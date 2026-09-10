//! The main process for communicating over IRC and managing state.
use std::sync::Arc;

use futures::stream::StreamExt;
use irc::client::prelude::Client;
use tracing::{debug, warn};

use crate::Error;
use crate::Registry;
use crate::config::Config;
use crate::plugin::{Context, PluginTask};

/// The main IRC bot struct that manages connection state and message handling.
pub struct Zeta {
    /// The complete configuration loaded from file or environment
    config: Config,
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
            config,
            registry,
            context,
        }
    }

    /// Starts the bot and begins processing IRC messages.
    ///
    /// Each plugin is spawned into its own long-lived task. Incoming messages are dispatched to
    /// every plugin through an unbounded channel, so a slow or failing plugin cannot block the IRC
    /// connection or the other plugins. Plugin errors, including failures during loading, are
    /// logged but never propagated.
    ///
    /// # Errors
    ///
    /// This function will return an error in the following situations:
    ///
    /// - [`Error::IrcClient`] - if the instantiation of the IRC client fails (e.g. due to
    ///   configuration issues.)
    /// - [`Error::IrcRegistration`] - if user registration fails (e.g. if the nickname is already taken.)
    /// - [`Error::Irc`] - if a protocol or communication error occurred.
    pub async fn run(&mut self) -> Result<(), Error> {
        let mut client = Client::from_config(self.config.irc.clone().into())
            .await
            .map_err(Error::IrcClient)?;

        client.identify().map_err(Error::IrcRegistration)?;

        let mut stream = client.stream()?;

        let context = Arc::clone(&self.context);
        let client = Arc::new(client);

        let mut plugins = self
            .registry
            .take_plugins()
            .into_iter()
            .map(|(name, plugin)| {
                PluginTask::spawn(name, plugin, Arc::clone(&context), Arc::clone(&client))
            })
            .collect::<Vec<_>>();

        while let Some(message) = stream.next().await.transpose()? {
            debug!(payload = %message, "processing irc message");

            let message = Arc::new(message);
            let mut index = 0;

            while index < plugins.len() {
                if let Err(error) = plugins[index].send(Arc::clone(&message)) {
                    warn!(plugin = %plugins[index].name, %error, "plugin task has stopped");

                    plugins.swap_remove(index);
                } else {
                    index += 1;
                }
            }
        }

        Ok(())
    }
}
