use irc::client::prelude::*;
use irc::proto::message::Tag;
use tokio::stream::StreamExt;

mod error;
pub use error::Error;

#[derive(Default)]
pub struct Core {
    client: Option<Client>,
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

    /// Continually polls for new IRC messages
    pub async fn poll(&mut self) -> Result<(), Error> {
        let mut stream = self
            .client
            .as_mut()
            .ok_or(Error::ClientNotConnectedError)?
            .stream()?;

        // Continually poll for new messages
        while let Some(message) = stream.next().await.transpose()? {
            // Handle the different kinds if commands and call the appropriate handler function
            match message.command {
                Command::PRIVMSG(ref target, ref msg) => {
                    self.handle_private_message(
                        message.source_nickname(),
                        message.tags.as_ref(),
                        target,
                        msg,
                    );
                }
                Command::JOIN(ref channel, _, _) => {
                    self.handle_join(message.source_nickname(), channel);
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Called when a message has been received
    fn handle_private_message(
        &self,
        sender: Option<&str>,
        _tags: Option<&Vec<Tag>>,
        target: &str,
        message: &str,
    ) -> Option<()> {
        println!("{} <{}> {}", target, sender.unwrap_or(""), message);

        Some(())
    }

    /// Handle that a user has entered a channel that we're also in
    fn handle_join(&self, sender: Option<&str>, channel: &str) {
        println!("{} joined {}", sender.unwrap_or(""), channel);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
