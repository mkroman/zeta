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
    fn new(_ctx: &Context) -> Result<Rink, ZetaError> {
        let ctx = rink_core::simple_context().map_err(plugin_err_display)?;
        let command = ZetaCommand::new(".r");

        Ok(Rink {
            ctx: Mutex::new(ctx),
            command,
        })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "rink".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
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
