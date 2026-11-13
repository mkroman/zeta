//! The IRC-to-`OTel` mapping of one incoming line.
//!
//! The host's `irc.dispatch` span and the runtime's `zeta.event.handle` spans describe the same
//! IRC line, so they must agree on the command, messaging operation, destination and message id.
//! [`MessageTelemetry`] computes that mapping from the dispatcher's own route classification,
//! and both span builders read it instead of re-deriving the values.
//!
//! The message is shared lazily with the events it produces, so one telemetry value cannot
//! outlive the [`SharedMessage`](crate::dispatch) mutations of the whole line. The dispatcher
//! therefore computes it once per delivery point — the command batch, the message/kind batch and
//! the URL batch — from the line's already-classified route.

use irc::proto::{Command, Message};
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
    pub command: &'a str,
    /// The `OTel` messaging operation, recorded as `messaging.operation.name`. Both spans also
    /// record `messaging.operation.type = "process"`.
    pub operation: MessagingOperation<'a>,
    /// The channel or nick the line targets, recorded as `irc.channel` and
    /// `messaging.destination.name`. [`None`] for commands without a destination (`QUIT`,
    /// `NICK`, raw commands).
    pub channel: Option<&'a str>,
    /// The IRCv3 `msgid` tag, recorded as `messaging.message.id`. [`None`] when the server
    /// attached no tag, which is the case on networks without the `message-tags` capability.
    pub message_id: Option<&'a str>,
}

impl<'a> MessageTelemetry<'a> {
    /// Computes the telemetry of `message`, or returns [`None`] when its command is not routed.
    ///
    /// This is the host-facing constructor: it classifies the command with the dispatcher's own
    /// route table, so `irc.dispatch` exists exactly for the lines the runtime handles. Server
    /// `ERROR` is the one addition — it is not routed, but its disconnect reason is logged
    /// inside the span.
    #[must_use]
    pub fn from_message(message: &'a Message) -> Option<Self> {
        if matches!(&message.command, Command::ERROR(..)) {
            return Some(Self {
                command: "ERROR",
                operation: MessagingOperation::Modeled("error"),
                channel: None,
                message_id: message_id(message),
            });
        }

        Self::from_route(CommandRoute::classify(&message.command), message)
    }

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

/// Returns whether the handler spans are enabled for the current subscriber.
///
/// The host's `irc.dispatch` span and the runtime's `zeta.event.handle` spans share the
/// `zeta.plugins` target, so this one filter check decides whether either span — and the
/// telemetry it needs — is built. The host calls it before creating a dispatch span, and the
/// dispatcher passes the result down to every delivery point.
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
