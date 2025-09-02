// Simple echo plugin implementation using the new Plugin trait directly
// This demonstrates how clean the new API is without macros

use async_trait::async_trait;
use serde::Deserialize;
use irc::proto::{Command, Message};
use irc::client::Client;
use tokio::time::{sleep, Duration};

use crate::plugin::{Plugin, PluginContext};
use crate::Error;

#[derive(Deserialize)]
pub struct EchoConfig {
    pub prefix: Option<String>,
}

pub struct Echo {
    prefix: String,
    _context: PluginContext,
}

#[async_trait]
impl Plugin for Echo {
    const NAME: &'static str = "echo";
    const AUTHOR: &'static str = "Mikkel Kroman <mk@maero.dk>";
    const VERSION: &'static str = "1.0.0";
    
    type Config = EchoConfig;
    
    async fn new(config: Self::Config, context: PluginContext) -> Result<Self, Error> {
        Ok(Echo {
            prefix: config.prefix.unwrap_or_else(|| ".echo ".to_string()),
            _context: context,
        })
    }
    
    async fn run(&mut self) -> Result<(), Error> {
        // Simple IRC-only plugins just stay alive
        loop {
            sleep(Duration::from_secs(60)).await;
        }
    }
    
    async fn handle_irc_message(&mut self, message: &Message, client: &Client) -> Result<(), Error> {
        if let Command::PRIVMSG(ref channel, ref msg) = message.command {
            if let Some(echo_text) = msg.strip_prefix(&self.prefix) {
                client
                    .send_privmsg(channel, format!("Echo: {}", echo_text))
                    .map_err(Error::IrcClientError)?;
            }
        }
        Ok(())
    }
}