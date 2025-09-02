use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use rand::prelude::IteratorRandom;
use serde::Deserialize;
use thiserror::Error;

use crate::Error as ZetaError;

use super::{Author, Version, NewPlugin};

#[derive(Error, Debug)]
pub enum Error {
    #[error("no valid options found")]
    NoOptions,
}

#[derive(Deserialize)]
pub struct ChoicesConfig {
    // No specific config needed for choices, but we need the struct
}

pub struct Choices;

#[async_trait]
impl NewPlugin for Choices {
    const NAME: &'static str = "choices";
    const AUTHOR: Author = Author("Mikkel Kroman <mk@maero.dk>");
    const VERSION: Version = Version("0.1.0");

    type Err = Error;
    type Config = ChoicesConfig;

    fn with_config(_config: &Self::Config) -> Self {
        Choices
    }

    async fn handle_message(&self, message: &Message, client: &Client, _ctx: &super::PluginContext) -> Result<(), ZetaError> {
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
                    .map_err(ZetaError::IrcClientError)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_should_strip_nick_prefix() {
        assert_eq!(
            strip_nick_prefix("zeta: hello world", "zeta"),
            Some("hello world")
        );
        assert_eq!(
            strip_nick_prefix("zeta, hello world", "zeta"),
            Some("hello world")
        );
    }

    #[test]
    fn it_should_not_extract_options_when_not_present() {
        assert_eq!(extract_options("hi"), None);
    }

    #[test]
    fn it_should_extract_options() {
        assert_eq!(extract_options("a eller b"), Some(vec!["a", "b"]));
        assert_eq!(
            extract_options("a, b, c eller d"),
            Some(vec!["a", "b", "c", "d"])
        );
    }

    #[test]
    fn it_should_extract_options_stripping_questionmark() {
        assert_eq!(extract_options("a eller b?"), Some(vec!["a", "b"]));
        assert_eq!(
            extract_options("a, b, c eller d?"),
            Some(vec!["a", "b", "c", "d"])
        );
    }
}
