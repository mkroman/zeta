//! Command events: a channel message that invoked a registered command.

use std::sync::Arc;

use irc::proto::{Command, Message};

use crate::command::CommandSpec;

use super::{Sender, message_id, privmsg};

/// A registered command was invoked in a channel.
///
/// The host matches the message against the [`CommandSpec`]s the plugin registered; the event
/// carries the specification that matched and the trailing, whitespace-normalized arguments.
#[derive(Clone, Debug)]
pub struct CommandEvent {
    pub(super) message: Arc<Message>,
    /// The command specification that matched.
    pub spec: CommandSpec,
    /// The byte offset of the arguments within the message text.
    args_start: usize,
}

impl CommandEvent {
    /// Creates a command event if `spec` matches the message's `PRIVMSG` text.
    ///
    /// Returns [`None`] when the message is not a `PRIVMSG` or its text does not match `spec`;
    /// the host only builds command events for the messages it routes to plugins.
    #[must_use]
    pub fn new(message: Arc<Message>, spec: CommandSpec) -> Option<Self> {
        let Command::PRIVMSG(_, text) = &message.command else {
            return None;
        };

        let args_start = spec.args_range(text)?.start;

        Some(Self {
            message,
            spec,
            args_start,
        })
    }

    /// Returns the channel the command was invoked in.
    #[must_use]
    pub fn channel(&self) -> &str {
        privmsg(&self.message).0
    }

    /// Returns the trailing arguments of the invocation, with leading whitespace stripped
    /// (empty when the command was invoked without arguments).
    #[must_use]
    pub fn args(&self) -> &str {
        &privmsg(&self.message).1[self.args_start..]
    }

    /// Returns the IRCv3 `msgid` of the invoking message, when it carries one.
    #[must_use]
    pub fn message_id(&self) -> Option<&str> {
        message_id(&self.message)
    }

    event_sender!();
}
