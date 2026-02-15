//! Helpful calculator features based on rink.

use std::sync::Mutex;

use rink_core::Context as RinkContext;

use crate::plugin::prelude::*;

/// Calculator plugin using rink-rs.
pub struct Rink {
    /// Handle to our rink context
    ctx: Mutex<RinkContext>,
    /// Handler for the `.r` command
    command: ZetaCommand,
}

#[async_trait]
impl Plugin<Context> for Rink {
    fn new(_ctx: &Context) -> Rink {
        let ctx = rink_core::simple_context().expect("could not create rink context");
        let command = ZetaCommand::new(".r");

        Rink {
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

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
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

impl Rink {
    pub fn eval(&self, line: &str) -> Result<String, String> {
        let mut ctx = self.ctx.lock().unwrap();

        rink_core::one_line(&mut ctx, line)
    }
}
