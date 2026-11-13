//! The IRC-to-`OTel` mapping of one incoming line.
//!
//! The runtime owns both spans of an IRC line: `EventIndex::dispatch` creates the line's
//! `irc.dispatch` span, and every delivery creates a `zeta.event.handle` span as its child. Both
//! describe the same line, so they must agree on the command, messaging operation, destination
//! and message id. [`MessageTelemetry`] computes that mapping from the dispatcher's own route
//! classification, and both span builders read it instead of re-deriving the values.
//!
//! The host keeps only the operational `irc.error` span: a server `ERROR` is not routed, so it
//! never reaches the dispatcher and has no messaging telemetry.
//!
//! The message is shared lazily with the events it produces, so one telemetry value cannot
//! outlive the [`SharedMessage`](crate::dispatch) mutations of the whole line. The dispatcher
//! wraps the message without sharing it, builds the line span from it and drops that telemetry
//! before routing shares the message, then computes the delivery telemetry once per delivery
//! point — the command batch, the message/kind batch and the URL batch — from the line's
//! already-classified route.
//!
//! The module is private, so the `pub` items below are internal to the runtime crate; none of
//! them is part of the `PluginRuntime` API.

use irc::proto::{Command, Message};
use tracing::Span;
use zeta_plugin::event::{MessagingOperation, message_id};

use crate::dispatch::CommandRoute;

/// The telemetry of one IRC line: the values both span builders record.
///
/// Each field feeds the same attribute on `irc.dispatch` and `zeta.event.handle`, so the two
/// spans cannot disagree about one line. Command, operation and channel come from the line's
/// route, already classified by the dispatcher; the IRCv3 message id is read from the message.
#[derive(Clone, Copy, Debug)]
pub struct MessageTelemetry<'a> {
    /// The IRC command as received, recorded as `irc.command` — `PRIVMSG`, `NOTICE`, `JOIN`, …
    /// and the received name for a raw command.
    pub(crate) command: &'a str,
    /// The `OTel` messaging operation, recorded as `messaging.operation.name`. Both spans also
    /// record `messaging.operation.type = "process"`.
    pub(crate) operation: MessagingOperation<'a>,
    /// The channel or nick the line targets, recorded as `irc.channel` and
    /// `messaging.destination.name`. [`None`] for commands without a destination (`QUIT`,
    /// `NICK`, raw commands).
    pub(crate) channel: Option<&'a str>,
    /// The IRCv3 `msgid` tag, recorded as `messaging.message.id`. [`None`] when the server
    /// attached no tag, which is the case on networks without the `message-tags` capability.
    pub(crate) message_id: Option<&'a str>,
}

impl<'a> MessageTelemetry<'a> {
    /// Computes the telemetry of a line already classified as `route`.
    ///
    /// The dispatcher classifies each line once and passes the route to every delivery point, so
    /// the command/operation mapping is not repeated per event. Returns [`None`] for
    /// [`CommandRoute::Ignored`] and for routes without an IRC command of their own.
    pub(crate) fn from_route(route: CommandRoute, message: &'a Message) -> Option<Self> {
        let (command, operation) = route.command_operation(&message.command)?;

        Some(Self {
            command,
            operation,
            channel: channel(&message.command),
            message_id: message_id(message),
        })
    }
}

/// Builds the `irc.dispatch` span of one delivering IRC line.
///
/// The dispatcher creates and enters this span before routing, so every handler span of the line
/// is its child and a line no subscription can receive leaves no trace. The span targets
/// `zeta.plugins`, like the handler spans, so one filter decision enables the whole message
/// trace. Its fields come from [`MessageTelemetry`], so the two spans cannot disagree about the
/// command, operation, destination or message id.
pub fn dispatch_span(telemetry: &MessageTelemetry<'_>) -> Span {
    let span = tracing::info_span!(
        target: "zeta.plugins",
        "irc.dispatch",
        irc.command = telemetry.command,
        irc.channel = tracing::field::Empty,
        otel.kind = "consumer",
        messaging.system.name = "irc",
        messaging.operation.name = %telemetry.operation,
        messaging.operation.type = "process",
        messaging.destination.name = tracing::field::Empty,
        messaging.message.id = tracing::field::Empty,
    );

    if let Some(channel) = telemetry.channel {
        span.record("irc.channel", channel);
        span.record("messaging.destination.name", channel);
    }

    if let Some(message_id) = telemetry.message_id {
        span.record("messaging.message.id", message_id);
    }

    span
}

/// Returns whether the handler spans are enabled for the current subscriber.
///
/// The line span and the handler spans share the `zeta.plugins` target, so this one filter check
/// decides whether either span — and the telemetry it needs — is built. The dispatcher calls it
/// once per line that may deliver, before the span is built and before routing shares the
/// message, and passes the result to every delivery point.
#[must_use]
pub fn handler_spans_enabled() -> bool {
    tracing::enabled!(target: "zeta.plugins", tracing::Level::INFO)
}

/// Returns the channel or nick a routed command targets, when it has one.
fn channel(command: &Command) -> Option<&str> {
    match command {
        Command::PRIVMSG(target, _)
        | Command::NOTICE(target, _)
        | Command::JOIN(target, ..)
        | Command::PART(target, ..)
        | Command::KICK(target, ..) => Some(target),
        _ => None,
    }
}
