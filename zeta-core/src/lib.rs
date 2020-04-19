use irc::client::prelude::*;
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
        //if self.client.is_none() {
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

    pub async fn poll(&mut self) -> Result<(), Error> {
        let mut stream = self
            .client
            .as_mut()
            .ok_or(Error::ClientNotConnectedError)?
            .stream()?;

        while let Some(message) = stream.next().await.transpose()? {
            print!("{}", message);
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
