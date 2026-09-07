//! Helpful calculator features based on rink.

use std::sync::Mutex;

use rink_core::Context as RinkContext;

use crate::plugin::prelude::*;

/// Calculator plugin using rink-rs.
pub struct Rink {
    /// Handle to our rink context
    ctx: Mutex<RinkContext>,
}

#[async_trait]
impl Plugin<Context> for Rink {
    fn new(_ctx: &Context) -> Result<Rink, ZetaError> {
        let ctx = rink_core::simple_context().map_err(|e| ZetaError::Plugin(Box::new(std::io::Error::other(e))))?;

        Ok(Rink {
            ctx: Mutex::new(ctx),
        })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "rink".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn commands(&self) -> &'static [Prefix] {
        const { &[Prefix::new(".r")] }
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        channel: &str,
        _command: &Prefix,
        query: &str,
    ) -> Result<(), ZetaError> {
        let message = match self.eval(query) {
            Ok(result) => format!("\x0310> {result}"),
            Err(err) => format!("\x0310> Error: {err}"),
        };

        client.send_privmsg(channel, message)?;

        Ok(())
    }
}

impl Rink {
    pub fn eval(&self, line: &str) -> Result<String, String> {
        let mut ctx = self.ctx.lock().unwrap();

        rink_core::one_line(&mut ctx, line)
    }
}
