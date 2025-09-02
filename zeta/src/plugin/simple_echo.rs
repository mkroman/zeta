//! Simplified echo plugin demonstrating the direct Plugin trait approach

use async_trait::async_trait;
use serde::Deserialize;
use irc::proto::{Command, Message};
use irc::client::Client;
use crate::plugin::{Plugin, PluginContext};
use crate::Error;

#[derive(Deserialize)]
pub struct EchoConfig {
    pub prefix: Option<String>,
}

pub struct SimpleEcho {
    prefix: String,
}

#[async_trait]
impl Plugin for SimpleEcho {
    const NAME: &'static str = "simple_echo";
    const AUTHOR: &'static str = "Demo";
    const VERSION: &'static str = "1.0.0";
    
    type Config = EchoConfig;
    
    async fn new(config: Self::Config, _context: PluginContext) -> Result<Self, Error> {
        Ok(SimpleEcho {
            prefix: config.prefix.unwrap_or_else(|| ".echo ".to_string()),
        })
    }
    
    async fn run(&mut self) -> Result<(), Error> {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    }
    
    async fn handle_irc_message(&mut self, message: &Message, client: &Client) -> Result<(), Error> {
        if let Command::PRIVMSG(ref channel, ref msg) = message.command {
            if let Some(echo_text) = msg.strip_prefix(&self.prefix) {
                client
                    .send_privmsg(channel, format!("🔊 {}", echo_text))
                    .map_err(Error::IrcClientError)?;
            }
        }
        Ok(())
    }
}

// Single line registration
crate::auto_plugin!(
    SimpleEcho,
    name = "simple_echo",
    author = "Demo",
    version = "1.0.0",
    config = EchoConfig
);