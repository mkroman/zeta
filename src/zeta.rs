//! The main process for communicating over IRC and managing state.
use futures::stream::StreamExt;
use irc::client::prelude::Client;
use irc::proto::Message;
use tracing::debug;

use crate::config::Config;
use crate::Error;
use crate::Registry;

pub struct Zeta {
    /// The complete configuration.
    config: Config,
    /// The IRC client once we want to connect.
    client: Option<Client>,
    /// The plugin registry.
    registry: Registry,
}

impl Zeta {
    pub fn from_config(config: Config) -> Result<Self, Error> {
        let registry = Registry::preloaded();

        Ok(Zeta {
            client: None,
            registry,
            config,
        })
    }

    /// Continually poll for messages and react to them.
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

    async fn handle_message(&self, client: &Client, message: Message) -> Result<(), Error> {
        debug!(?message);

        for plugin in &self.registry.plugins {
            plugin.handle_message(&message, client).await?;
        }

        Ok(())
    }
}
