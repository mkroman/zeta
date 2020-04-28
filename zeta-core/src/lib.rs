use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use irc::client::prelude::*;
use log::{debug, info};
use tokio::stream::StreamExt;

mod channel;
pub mod config;
mod error;
mod user;

pub use channel::Channel;
pub use config::{Config, NetworkConfig};
pub use error::Error;
pub use user::User;

#[derive(Default)]
pub struct Core {
    client: Option<Client>,
    channels: HashMap<String, Arc<RwLock<Channel>>>,
    users: HashMap<String, Arc<RwLock<User>>>,
}

impl Core {
    pub fn new() -> Core {
        Core {
            ..Default::default()
        }
    }

    pub async fn connect<C: Into<config::IrcConfig>>(&mut self, config: C) -> Result<(), Error> {
        let client = Client::from_config(config.into().0).await?;
        client.identify()?;
        self.client = Some(client);

        Ok(())
    }

    /// Continually polls for new IRC messages
    pub async fn poll(&mut self) -> Result<(), Error> {
        let mut stream = self
            .client
            .as_mut()
            .ok_or(Error::ClientNotConnectedError)?
            .stream()?;

        // Continually poll for new messages
        while let Some(message) = stream.next().await.transpose()? {
            debug!("<< {:?}", message.command);

            match message.command {
                Command::Response(Response::RPL_ISUPPORT, ref args) => {
                    self.handle_isupport(args);
                }
                Command::Response(Response::RPL_NAMREPLY, ref args) => {
                    debug!("NAMES: {:?}", args);
                }
                Command::PRIVMSG(ref _target, ref _msg) => {
                    // Find the senders nickname
                    if let Some(nick) = message.source_nickname() {
                        // Create an new user instance and insert it if it doesn't already exist
                        if !self.users.contains_key(nick) {
                            self.users
                                .insert(nick.to_string(), Arc::new(RwLock::new(User::new(nick))));
                        }

                        // Find the user entry if it exists
                        let _user = self.users.get(nick);
                    }
                }
                Command::JOIN(ref chan_name, _, _) => {
                    if message.source_nickname()
                        == Some(self.client.as_ref().unwrap().current_nickname())
                    {
                        self.channels.insert(
                            chan_name.into(),
                            Arc::new(RwLock::new(Channel::new(chan_name))),
                        );

                        info!("Joined `{}'", chan_name);
                    }

                    let _channel = self.channels.get(chan_name);
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Handles the ISUPPORT message that is sent by the server to inform the
    /// client about features that might differ across server implementations
    fn handle_isupport(&mut self, args: &[String]) {
        println!("ISUPPORT: {:?}", args);
    }
}
