//! CTCP events: client-to-client protocol requests and replies.

use std::sync::Arc;

use irc::proto::{Command, Message};

use super::{FromMessage, Sender};

/// Destructures a message into its target and text, whether it is a `PRIVMSG` or a `NOTICE`.
fn message_text(message: &Message) -> Option<(&str, &str)> {
    match &message.command {
        Command::PRIVMSG(target, text) | Command::NOTICE(target, text) => Some((target, text)),
        _ => None,
    }
}

/// A CTCP request or reply arrived.
///
/// Any `PRIVMSG` or `NOTICE` whose text is wrapped in CTCP `\x01` markers produces one event —
/// so such messages are never delivered as [`MessageEvent`](super::MessageEvent)s or
/// [`UrlEvent`](super::UrlEvent)s.
#[derive(Clone, Debug)]
pub struct CtcpEvent {
    pub(super) message: Arc<Message>,
    /// The kind of CTCP message.
    pub kind: CtcpKind,
}

impl CtcpEvent {
    /// Creates a CTCP event if `message` carries a CTCP payload.
    #[must_use]
    pub fn new(message: Arc<Message>) -> Option<Self> {
        let (_, text) = message_text(&message)?;
        let (command, _) = parse_ctcp(text)?;

        Some(Self {
            kind: CtcpKind::classify(command),
            message,
        })
    }

    /// Returns the target the CTCP message was sent to: the channel, or the recipient's
    /// nickname for direct queries.
    ///
    /// # Panics
    ///
    /// Panics if the underlying message is not a `PRIVMSG` or `NOTICE`; the event is only ever
    /// delivered for those.
    #[must_use]
    pub fn target(&self) -> &str {
        message_text(&self.message)
            .expect("ctcp events are only delivered for PRIVMSG or NOTICE")
            .0
    }

    /// Returns the full, `\x01`-wrapped text of the CTCP message.
    ///
    /// # Panics
    ///
    /// Panics if the underlying message is not a `PRIVMSG` or `NOTICE`; the event is only ever
    /// delivered for those.
    #[must_use]
    pub fn text(&self) -> &str {
        message_text(&self.message)
            .expect("ctcp events are only delivered for PRIVMSG or NOTICE")
            .1
    }

    /// Returns the CTCP command, e.g. `ACTION`.
    #[must_use]
    pub fn command(&self) -> &str {
        parse_ctcp(self.text()).map_or("", |(command, _)| command)
    }

    /// Returns the CTCP payload following the command (empty when there is none).
    #[must_use]
    pub fn args(&self) -> &str {
        parse_ctcp(self.text()).map_or("", |(_, args)| args)
    }

    event_sender!();
}

impl FromMessage for CtcpEvent {
    fn from_message(message: Arc<Message>) -> Option<Self> {
        Self::new(message)
    }
}

/// The kind of a [`CtcpEvent`], classified from its CTCP command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CtcpKind {
    /// An action (`/me`) message.
    Action,
    /// A direct client-to-client connection request.
    Dcc,
    /// A latency probe.
    Ping,
    /// A client version query.
    Version,
    /// A local time query.
    Time,
    /// A query for the supported CTCP commands.
    Clientinfo,
    /// A client information query.
    Userinfo,
    /// Any other CTCP command.
    Other,
}

impl CtcpKind {
    /// Classifies a CTCP command, matching case-insensitively.
    #[must_use]
    pub const fn classify(command: &str) -> Self {
        // Compared in place: uppercasing the command would allocate a `String` for every
        // incoming CTCP message, and `/me` (`ACTION`) is common.
        if command.eq_ignore_ascii_case("ACTION") {
            Self::Action
        } else if command.eq_ignore_ascii_case("DCC") {
            Self::Dcc
        } else if command.eq_ignore_ascii_case("PING") {
            Self::Ping
        } else if command.eq_ignore_ascii_case("VERSION") {
            Self::Version
        } else if command.eq_ignore_ascii_case("TIME") {
            Self::Time
        } else if command.eq_ignore_ascii_case("CLIENTINFO") {
            Self::Clientinfo
        } else if command.eq_ignore_ascii_case("USERINFO") {
            Self::Userinfo
        } else {
            Self::Other
        }
    }
}

/// Returns whether `message` carries a CTCP payload.
///
/// The dispatcher and [`CtcpEvent::new`] share this predicate, so the rule for which
/// `PRIVMSG`/`NOTICE` texts are CTCP messages cannot drift between routing and construction.
#[must_use]
pub fn is_ctcp(message: &Message) -> bool {
    message_text(message).is_some_and(|(_, text)| parse_ctcp(text).is_some())
}

/// Splits the payload of a CTCP message into its command and arguments.
///
/// The text is `\x01`-wrapped (e.g. `"\x01ACTION slaps\x01"`); a missing trailing `\x01` is
/// tolerated, as some clients omit it.
fn parse_ctcp(text: &str) -> Option<(&str, &str)> {
    let body = text.strip_prefix('\x01')?;
    let body = body.trim_suffix('\x01');

    Some(match body.split_once(' ') {
        Some((command, args)) => (command, args),
        None => (body, ""),
    })
}
