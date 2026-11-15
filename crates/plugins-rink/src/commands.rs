//! The evaluation path behind the `.r` and `.c` commands.
//!
//! Both commands evaluate their whole argument as a rink expression and reply with the
//! one-line result; `.c` case-corrects the unit tokens rink cannot resolve first. A query
//! that runs into missing currency data refreshes what can still change the outcome and
//! retries once, mirroring upstream rink's REPL.

use zeta_plugin::irc::send_reply;
use zeta_plugin::{Client, CommandEvent, ZetaError, plugin_err};

use super::Rink;
use super::actor::{EvalOutcome, Rewrite};

impl Rink {
    /// Replies to `.r` or `.c` with the evaluated calculation.
    pub(super) async fn reply_calculation(
        &self,
        client: &Client,
        command: &CommandEvent,
        rewrite: Rewrite,
    ) -> Result<(), ZetaError> {
        let message = self.eval_and_format(command.args(), rewrite).await?;

        send_reply(client, command.channel(), message)?;

        Ok(())
    }

    /// Evaluates `line` and renders the result or error as a one-line IRC reply.
    ///
    /// With [`Rewrite::Classify`], unit tokens rink cannot resolve are case-corrected against
    /// the registry first, so `.c 10 usd to dkk` evaluates like `10 USD to DKK`.
    ///
    /// A query that runs into missing currency data triggers an inline refresh and a
    /// re-evaluation, mirroring upstream rink's REPL, which fetches on demand in the same
    /// situation. The refresh is only attempted when it can actually change the outcome —
    /// the cached dataset is missing, stale, or lacks a requested cryptocurrency table — so
    /// a dependency that stays unresolvable does not fetch on every query. A failed refresh
    /// is logged and the missing-dependencies error is answered instead.
    ///
    /// # Errors
    ///
    /// Returns a [`ZetaError`] when the rink actor has stopped.
    pub(super) async fn eval_and_format(
        &self,
        line: &str,
        rewrite: Rewrite,
    ) -> Result<String, ZetaError> {
        let actor = self
            .actor
            .as_ref()
            .ok_or_else(|| plugin_err(std::io::Error::other("the rink actor is not running")))?;

        let retry_line = match actor.eval(line.to_owned(), rewrite).await? {
            EvalOutcome::Answer(text) => return Ok(text),
            EvalOutcome::NeedsCurrency { error_reply, .. } if self.client.is_none() => {
                return Ok(error_reply);
            }
            EvalOutcome::NeedsCurrency { retry_line, .. } => retry_line,
        };

        // Currency data is missing: refresh what can still change the outcome, then
        // re-evaluate the same classified line, mirroring upstream rink's REPL, which fetches
        // on demand in the same situation.
        self.ensure_currency_cached().await;

        match actor.eval(retry_line, Rewrite::Verbatim).await? {
            EvalOutcome::Answer(text) => Ok(text),
            EvalOutcome::NeedsCurrency { error_reply, .. } => Ok(error_reply),
        }
    }
}
