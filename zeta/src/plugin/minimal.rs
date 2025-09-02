//! Ultra-minimal plugin demonstrating the extreme code reduction possible

use async_trait::async_trait;
use serde::Deserialize;
use irc::proto::{Command, Message};
use irc::client::Client;
use crate::plugin::{Plugin, PluginContext};
use crate::Error;

#[derive(Deserialize)]
pub struct MinimalConfig;

pub struct Minimal;

#[async_trait]
impl Plugin for Minimal {
    const NAME: &'static str = "minimal";
    const AUTHOR: &'static str = "Demo";
    const VERSION: &'static str = "1.0.0";
    
    type Config = MinimalConfig;
    
    async fn new(_config: Self::Config, _context: PluginContext) -> Result<Self, Error> {
        Ok(Minimal)
    }
    
    async fn run(&mut self) -> Result<(), Error> {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    }
    
    async fn handle_irc_message(&mut self, message: &Message, client: &Client) -> Result<(), Error> {
        if let Command::PRIVMSG(ref channel, ref msg) = message.command {
            if msg == ".minimal" {
                client
                    .send_privmsg(channel, "✨ Minimal plugin works!")
                    .map_err(Error::IrcClientError)?;
            }
        }
        Ok(())
    }
}

// This is ALL that's needed for registration!
crate::auto_plugin!(
    Minimal,
    name = "minimal",
    author = "Demo",
    version = "1.0.0",
    config = MinimalConfig
);