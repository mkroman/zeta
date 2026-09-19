//! Evaluates calculations with unit conversions through rink.
//!
//! The `.r <expression>` command evaluates the whole argument as a rink expression and replies
//! with the one-line result as a notice — arithmetic, physical unit conversions, currency
//! lookups (via the bundled dataset), and more. Evaluation errors are reported inline as
//! `Error: <message>`.
//!
//! A rink context is built once at plugin initialization; evaluation calls are serialized
//! through it, and a failed context build aborts plugin initialization. The plugin has no
//! settings.

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
    /// Evaluates `line` as a rink expression and returns the one-line result.
    ///
    /// # Errors
    ///
    /// Returns the rink error message when the expression cannot be evaluated, e.g. on a
    /// syntax error or an unknown unit.
    ///
    /// # Panics
    ///
    /// Panics if the rink context mutex is poisoned, which can only happen if an evaluation
    /// panicked while holding the lock.
    pub fn eval(&self, line: &str) -> Result<String, String> {
        let mut ctx = self.ctx.lock().unwrap();

        rink_core::one_line(&mut ctx, line)
    }
}
