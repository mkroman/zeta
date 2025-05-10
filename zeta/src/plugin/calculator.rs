use std::sync::Mutex;

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};

use crate::Error as ZetaError;

use super::{Author, Name, Plugin, Version};

pub struct Calculator {
    ctx: Mutex<rink_core::Context>,
}

#[async_trait]
impl Plugin for Calculator {
    fn new() -> Calculator {
        let ctx = rink_core::simple_context().expect("could not create rink-rs context");

        Calculator {
            ctx: Mutex::new(ctx),
        }
    }

    fn name() -> Name {
        Name("calculator")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref inner_message) = message.command {
            if let Some(query) = inner_message.strip_prefix(".r ") {
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
        }

        Ok(())
    }
}

impl Calculator {
    pub fn eval(&self, line: &str) -> Result<String, String> {
        let mut ctx = self.ctx.lock().unwrap();

        rink_core::one_line(&mut ctx, line)
    }
}
