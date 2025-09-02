use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use rand::prelude::IteratorRandom;
use serde::Deserialize;
use tokio::time::{sleep, Duration};

use crate::plugin::{Plugin, PluginContext, MessageEnvelope};
use crate::Error;

#[derive(Deserialize)]
pub struct ChoicesConfig {
    // No specific config needed, but we need the struct
}

pub struct ChoicesV2 {
    context: PluginContext,
}

#[async_trait]
impl Plugin for ChoicesV2 {
    const NAME: &'static str = "choices_v2";
    const AUTHOR: &'static str = "Mikkel Kroman <mk@maero.dk>";
    const VERSION: &'static str = "2.0.0";
    
    type Config = ChoicesConfig;
    
    async fn new(_config: Self::Config, context: PluginContext) -> Result<Self, Error> {
        Ok(ChoicesV2 { context })
    }
    
    async fn run(&mut self) -> Result<(), Error> {
        // Simple IRC-only plugins can just keep alive
        loop {
            sleep(Duration::from_secs(60)).await;
        }
    }
    
    async fn handle_irc_message(&mut self, message: &Message, client: &Client) -> Result<(), Error> {
        if let Command::PRIVMSG(ref channel, ref inner_message) = message.command {
            let current_nickname = client.current_nickname();

            if let Some(msg) = strip_nick_prefix(inner_message, current_nickname)
                && let Some(options) = extract_options(msg)
            {
                let source_nickname = message.source_nickname().unwrap_or("");
                let mut rng = rand::thread_rng();
                let selection = options.iter().choose(&mut rng).unwrap();

                client
                    .send_privmsg(channel, format!("{source_nickname}: {selection}",))
                    .map_err(Error::IrcClientError)?;
            }
        }

        Ok(())
    }
}

fn strip_nick_prefix<'a>(s: &'a str, current_nickname: &'a str) -> Option<&'a str> {
    if let Some(s) = s.strip_prefix(current_nickname) {
        if s.starts_with(", ") || s.starts_with(": ") {
            Some(&s[2..])
        } else {
            None
        }
    } else {
        None
    }
}

fn extract_options(s: &str) -> Option<Vec<&str>> {
    let mut parts = s.splitn(2, " eller ");

    if let (Some(first), Some(last)) = (parts.next(), parts.next()) {
        let mut options: Vec<&str> = first.split(", ").collect();

        // If the last option ends with a question mark, skip it.
        if let Some(last) = last.strip_suffix('?') {
            options.push(last);
        } else {
            options.push(last);
        }

        return Some(options);
    }

    None
}