//! Command subscriptions and the classification of a line's IRC command.

use irc::proto::Command;

use zeta_plugin::event::MessagingOperation;
use zeta_plugin::{CommandSpec, EventKind};

use super::subscriber::Subscriber;

/// A registered command in the dispatch index.
#[derive(Clone, Debug)]
pub(super) struct CommandTarget {
    pub(super) subscriber: Subscriber,
    pub(super) spec: CommandSpec,
}

/// What [`EventIndex::dispatch`] should do with a message's IRC command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandRoute {
    /// A channel message: commands, messages, or URLs may match.
    Privmsg,
    /// A flag-based event kind.
    Event(EventKind),
    /// Connection protocol or another command no plugin can observe.
    Ignored,
}

impl CommandRoute {
    /// Classifies `command` by the events it can produce.
    pub(crate) const fn classify(command: &Command) -> Self {
        match command {
            Command::PRIVMSG(..) => Self::Privmsg,
            // CTCP replies arrive as NOTICE-wrapped messages; only CTCP subscribers see them —
            // ordinary notices are not an event kind.
            Command::NOTICE(..) => Self::Event(EventKind::Ctcp),
            Command::JOIN(..) => Self::Event(EventKind::Join),
            Command::PART(..) => Self::Event(EventKind::Part),
            Command::QUIT(..) => Self::Event(EventKind::Quit),
            Command::NICK(..) => Self::Event(EventKind::Nick),
            Command::KICK(..) => Self::Event(EventKind::Kick),
            Command::Raw(..) => Self::Event(EventKind::Raw),
            // Everything else is connection protocol the plugins have no events for.
            _ => Self::Ignored,
        }
    }

    /// Returns the IRC command name and `OTel` messaging operation of a routed command.
    ///
    /// The name is the command as received — uppercase for modeled commands, the received name
    /// for raw ones. The operation is the lowercased modeled command; an unmodeled command
    /// keeps its received name and is lowercased when the span field is rendered. [`None`] for
    /// [`Self::Ignored`] and for event kinds without an IRC command of their own.
    pub(crate) fn command_operation(
        self,
        command: &Command,
    ) -> Option<(&str, MessagingOperation<'_>)> {
        match self {
            Self::Privmsg => Some(("PRIVMSG", MessagingOperation::Modeled("privmsg"))),
            // A NOTICE is the only command routed as a CTCP event.
            Self::Event(EventKind::Ctcp) => Some(("NOTICE", MessagingOperation::Modeled("notice"))),
            Self::Event(EventKind::Join) => Some(("JOIN", MessagingOperation::Modeled("join"))),
            Self::Event(EventKind::Part) => Some(("PART", MessagingOperation::Modeled("part"))),
            Self::Event(EventKind::Quit) => Some(("QUIT", MessagingOperation::Modeled("quit"))),
            Self::Event(EventKind::Nick) => Some(("NICK", MessagingOperation::Modeled("nick"))),
            Self::Event(EventKind::Kick) => Some(("KICK", MessagingOperation::Modeled("kick"))),
            Self::Event(EventKind::Raw) => match command {
                Command::Raw(name, _) => Some((name, MessagingOperation::Unmodeled(name))),
                _ => None,
            },
            // `Message` has no IRC command of its own, and future kinds need a runtime that
            // knows how to route them.
            Self::Event(_) | Self::Ignored => None,
        }
    }
}
