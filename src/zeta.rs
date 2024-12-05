//! The main process for communicating over IRC and managing state.
use futures::stream::StreamExt;
use irc::client::prelude::{Client, Command};
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
        let registry = Registry::loaded();

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

        // self.irc = Some(client);

        // identify comes from ClientExt
        client.identify().map_err(Error::IrcRegistrationError)?;

        let mut stream = client.stream()?;

        while let Some(message) = stream.next().await.transpose()? {
            debug!(?message);

            if let Command::PRIVMSG(channel, message) = message.command {
                if message.contains(client.current_nickname()) {
                    // send_privmsg comes from ClientExt
                    client.send_privmsg(&channel, "beep boop").unwrap();
                }
            }
        }

        Ok(())
    }
}
