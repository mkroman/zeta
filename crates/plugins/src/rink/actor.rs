//! The actor task owning the rink context.
//!
//! A single task owns the context and serializes every evaluation through its mailbox, so
//! evaluation needs no lock — and because rink's replies hold `Rc`s and are not `Send`, each one
//! is rendered inside the task and only the resulting `String` crosses the channel. The currency
//! refresh task asks the same actor to swap in a rebuilt context, so a swap is ordered with the
//! evaluations around it.

use rink_core::Context as RinkContext;
use rink_core::output::QueryError;
use tokio::sync::{mpsc, oneshot};
use tracing::Instrument;

use crate::prelude::*;

use super::currency::CurrencyData;
use super::{error_message, render_reply};

/// A handle to the rink actor task.
#[derive(Clone)]
pub(super) struct Handle(mpsc::UnboundedSender<Request>);

/// The rendered outcome of an evaluation request.
pub(super) enum EvalOutcome {
    /// The final IRC reply, including the notice marker.
    Answer(String),
    /// The query needs currency data that is not loaded yet.
    NeedsCurrency {
        /// The missing-dependencies error, rendered as the error reply.
        error_reply: String,
        /// The classified line that was evaluated, for an unchanged retry.
        retry_line: String,
    },
}

/// A request to the rink actor.
enum Request {
    /// Classify the line when asked, evaluate it, and render the result.
    Evaluate {
        /// The line to evaluate.
        line: String,
        /// Whether to case-correct unresolvable unit tokens first.
        classify: bool,
        /// The channel the rendered answer is sent back on.
        reply: oneshot::Sender<EvalOutcome>,
    },
    /// Rebuild the context from freshly fetched currency data and swap it in.
    UpdateContext {
        /// The fetched currency data.
        data: CurrencyData,
        /// The channel the swap result is sent back on.
        reply: oneshot::Sender<Result<(), ZetaError>>,
    },
}

impl Handle {
    /// Spawns the actor task owning `ctx`.
    pub(super) fn spawn(ctx: RinkContext) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();

        tokio::spawn(run(ctx, rx).instrument(tracing::info_span!("rink_actor")));

        Self(tx)
    }

    /// Evaluates `line`, rendering the result inside the actor.
    ///
    /// # Errors
    ///
    /// Returns a [`ZetaError`] when the actor task has stopped.
    pub(super) async fn eval(&self, line: String, classify: bool) -> Result<EvalOutcome, ZetaError> {
        let (reply, response) = oneshot::channel();

        self.0
            .send(Request::Evaluate {
                line,
                classify,
                reply,
            })
            .map_err(|_| actor_stopped())?;

        response.await.map_err(|_| actor_stopped())
    }

    /// Asks the actor to rebuild the context from `data` and swap it in.
    ///
    /// # Errors
    ///
    /// Returns a [`ZetaError`] when the actor task has stopped or the context rebuild fails.
    pub(super) async fn update_context(&self, data: CurrencyData) -> Result<(), ZetaError> {
        let (reply, response) = oneshot::channel();

        self.0
            .send(Request::UpdateContext { data, reply })
            .map_err(|_| actor_stopped())?;

        response.await.map_err(|_| actor_stopped())?
    }
}

/// Returns the error reported when the actor task is gone.
fn actor_stopped() -> ZetaError {
    plugin_err(std::io::Error::other("the rink actor has stopped"))
}

/// Runs the actor loop until every handle is dropped.
async fn run(mut ctx: RinkContext, mut rx: mpsc::UnboundedReceiver<Request>) {
    while let Some(request) = rx.recv().await {
        match request {
            Request::Evaluate {
                line,
                classify,
                reply,
            } => {
                let evaluated_line = if classify {
                    super::classify(&ctx, &line)
                } else {
                    line
                };

                // Rink's replies are not `Send`, so the answer is rendered here, before it is
                // sent back over the channel.
                let answer = match rink_core::eval(&mut ctx, &evaluated_line) {
                    Ok(reply) => EvalOutcome::Answer(notice(render_reply(&reply))),
                    Err(error @ QueryError::MissingDeps(_)) => EvalOutcome::NeedsCurrency {
                        error_reply: notice(error_message(&error)),
                        retry_line: evaluated_line,
                    },
                    Err(error) => EvalOutcome::Answer(notice(error_message(&error))),
                };

                let _ = reply.send(answer);
            }
            Request::UpdateContext { data, reply } => {
                let _ = reply.send(super::currency::swap_in(&mut ctx, &data));
            }
        }
    }
}
