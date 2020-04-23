use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use irc::client::prelude::*;
use log::debug;
use tokio::stream::StreamExt;

mod channel;
mod error;
mod user;

pub use channel::Channel;
pub use error::Error;
pub use user::User;

#[derive(Default)]
pub struct Core {
    client: Option<Client>,
    channels: HashMap<String, Arc<RwLock<Channel>>>,
    users: Vec<Arc<User>>,
}

impl Core {
    pub fn new() -> Core {
        Core {
            ..Default::default()
        }
    }

    pub async fn connect(&mut self) -> Result<(), Error> {
        let config = Config {
            nickname: Some("zeta".to_owned()),
            server: Some("irc.uplink.io".to_owned()),
            channels: vec!["#test".to_owned()],
            ..Config::default()
        };

        let client = Client::from_config(config).await?;
        client.identify()?;
        self.client = Some(client);

        Ok(())
    }

    /// Attempts to find a user on the network with the given `nick` and returns a reference to it.
    /// If the user is not found, None is returned
    fn find_user_by_nick(&self, nick: &str) -> Option<Arc<User>> {
        self.users.iter().find(|user| user.nick() == nick).cloned()
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
            debug!("<< {}", message.to_string().trim());

            match message.command {
                Command::PRIVMSG(ref _target, ref _msg) => {
                    // Find the senders nickname
                    if let Some(nick) = message.source_nickname() {
                        // Find the user entry if it exists
                        if let Some(user) = self.find_user_by_nick(nick) {
                            debug!("Found existing user: {:?}", user)
                        } else {
                            // Create a new user entry
                        }
                    }
                }
                Command::JOIN(ref chan_name, _, _) => {
                    if !self.channels.contains_key(chan_name) {
                        self.channels.insert(
                            chan_name.into(),
                            Arc::new(RwLock::new(Channel::new(chan_name))),
                        );
                    }

                    let channel = self.channels.get(chan_name);
                }
                _ => {}
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
