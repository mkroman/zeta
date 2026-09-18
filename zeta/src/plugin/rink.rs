//! Helpful calculator features based on rink.

use std::sync::Mutex;

use rink_core::Context as RinkContext;

use crate::plugin::prelude::*;

/// The `.r` command.
const RINK: CommandSpec = CommandSpec::new(
    ".r",
    "Evaluate a calculation with unit conversions",
);

/// Calculator plugin using rink-rs.
pub struct Rink {
    /// Handle to our rink context
    ctx: Mutex<RinkContext>,
}

#[async_trait]
impl Plugin<Context> for Rink {
    type Settings = NoSettings;

    fn new(_ctx: &Context, _settings: &NoSettings, subscriptions: &mut Subscriptions) -> Result<Rink, ZetaError> {
        subscriptions.command(RINK);
        let ctx = rink_core::simple_context()
            .map_err(|e| ZetaError::Plugin(Box::new(std::io::Error::other(e))))?;

        Ok(Rink {
            ctx: Mutex::new(ctx),
        })
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let message = match self.eval(command.args()) {
            Ok(result) => notice(result),
            Err(err) => notice(format!("Error: {err}")),
        };

        client.send_privmsg(command.channel(), message)?;

        Ok(())
    }
}

impl Rink {
    pub fn eval(&self, line: &str) -> Result<String, String> {
        let mut ctx = self.ctx.lock().unwrap();

        rink_core::one_line(&mut ctx, line)
    }
}
