//! Helpful calculator features.

use std::sync::Mutex;

use rink_core::Context;

use crate::plugin::prelude::*;

/// Calculator plugin using rink-rs.
pub struct Calculator {
    /// Handle to our rink context
    ctx: Mutex<Context>,
    /// Handler for the `.r` command
    command: ZetaCommand,
}

#[async_trait]
impl Plugin for Calculator {
    fn new() -> Calculator {
        let ctx = rink_core::simple_context().expect("could not create rink context");
        let command = ZetaCommand::new(".r");

        Calculator {
            ctx: Mutex::new(ctx),
            command,
        }
    }

    fn name() -> Name {
        Name::from("choices")
    }

    fn author() -> Author {
        Author::from("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version::from("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(query) = self.command.parse(user_message)
        {
            let message = match self.eval(query) {
                Ok(result) => format!("\x0310> {result}"),
                Err(err) => format!("\x0310> Error: {err}"),
            };

            client.send_privmsg(channel, message)?;
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
