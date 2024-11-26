//! The main process for communicating over IRC and managing state.
use irc::client::prelude::Client as IrcClient;

use crate::config::Config;

pub struct Zeta {
    /// The IRC client.
    irc: IrcClient,
}

impl Zeta {
    pub async fn from_config(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        let client = IrcClient::from_config(config.irc.into()).await?;

        Ok(Zeta { irc: client })
    }
}
