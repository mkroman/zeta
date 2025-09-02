use std::sync::Mutex;

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use serde::Deserialize;
use thiserror::Error;

use crate::Error as ZetaError;

use super::{Author, Version, NewPlugin};

#[derive(Error, Debug)]
pub enum Error {
    #[error("evaluation error: {0}")]
    Evaluation(String),
    #[error("could not create rink context")]
    Context,
}

#[derive(Deserialize)]
pub struct CalculatorConfig {
    // No specific config needed for calculator, but we need the struct
}

pub struct Calculator {
    ctx: Mutex<rink_core::Context>,
}

#[async_trait]
impl NewPlugin for Calculator {
    const NAME: &'static str = "calculator";
    const AUTHOR: Author = Author("Mikkel Kroman <mk@maero.dk>");
    const VERSION: Version = Version("0.1.0");

    type Err = Error;
    type Config = CalculatorConfig;

    fn with_config(_config: &Self::Config) -> Self {
        let ctx = rink_core::simple_context().expect("could not create rink-rs context");

        Calculator {
            ctx: Mutex::new(ctx),
        }
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref inner_message) = message.command
            && let Some(query) = inner_message.strip_prefix(".r ")
        {
            match self.eval(query) {
                Ok(result) => {
                    client
                        .send_privmsg(channel, format!("\x0310> {result}"))
                        .map_err(ZetaError::IrcClientError)?;
                }
                Err(err) => {
                    client
                        .send_privmsg(channel, format!("\x0310> Error: {err}"))
                        .map_err(ZetaError::IrcClientError)?;
                }
            }
        }

        Ok(())
    }
}

impl Calculator {
    pub fn eval(&self, line: &str) -> Result<String, Error> {
        let mut ctx = self.ctx.lock().unwrap();

        rink_core::one_line(&mut ctx, line).map_err(Error::Evaluation)
    }
}
