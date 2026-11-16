//! Zeta-specific events delivered to plugins.
//!
//! Instead of raw IRC protocol messages, plugins receive events — typed values derived from the
//! incoming IRC traffic they registered an interest in during initialization. A plugin declares
//! its interests in [`Registered::new`](crate::Registered::new) through a [`Subscriptions`]
//! set;
//! the host only routes messages that match it, so a plugin's handlers are called only for
//! the events it asked for.
//!
//! Every event borrows its data from the IRC message it was derived from: the message is shared
//! behind an [`Arc`], and the event accessors (`channel()`, `text()`, `sender()`, …) return
//! borrowed views. Derived values such as command arguments are computed lazily from the shared
//! message, so delivering an event only clones an [`Arc`], never a string.
//!
//! Events are constructed by the host's dispatcher; the public constructors exist so tests (and
//! plugin authors) can build events to exercise handlers directly.
//!
//! The module is split by concern: the `command`, `url`, `message`, `presence`, `ctcp` and
//! `raw` children hold the typed events, and `subscriptions` holds the [`Event`] set, its
//! [`EventKind`] flags and the [`Subscriptions`] a plugin registers.

use std::borrow::Cow;
use std::fmt::{self, Write as _};
use std::sync::Arc;

use irc::proto::message::Tag;
use irc::proto::{Command, Message};

/// Destructures a message into its `PRIVMSG` target and text.
///
/// The host only constructs `PRIVMSG`-derived events for matching messages; reaching another
/// command here is a host bug.
fn privmsg(message: &Message) -> (&str, &str) {
    match &message.command {
        Command::PRIVMSG(target, text) => (target, text),
        _ => unreachable!("events are only delivered for PRIVMSG messages"),
    }
}

/// Returns the IRCv3 `msgid` tag of a message, when it carries one.
///
/// The tag is read from any message: servers may attach it to any event, and normally do so for
/// `PRIVMSG` and `NOTICE`. Message tags only reach clients that negotiated the `message-tags`
/// capability; the client requests it at registration, but a network that does not advertise it
/// sends no tags.
#[must_use]
pub fn message_id(message: &Message) -> Option<&str> {
    message
        .tags
        .as_deref()?
        .iter()
        .find(|Tag(key, _)| key.as_str() == "msgid")
        .and_then(|Tag(_, value)| value.as_deref())
}

/// Returns the IRCv3 `account` tag of a message, when it carries a services account.
///
/// The tag is attached by the server to messages from logged-in users when the `account-tag`
/// capability is negotiated; the value is the account name the sender is identified with.
/// Some networks send the tag with a `*` value for users who are not logged in, which reads
/// as no account here, like an absent tag.
#[must_use]
pub fn account(message: &Message) -> Option<&str> {
    let account = message
        .tags
        .as_deref()?
        .iter()
        .find(|Tag(key, _)| key.as_str() == "account")
        .and_then(|Tag(_, value)| value.as_deref())?;

    (account != "*" && !account.is_empty()).then_some(account)
}

/// The sender of a message, as identified by its IRC prefix.
///
/// Events without a nickname prefix (e.g. server notices) have no sender; sender-scoped
/// filters never match without one.
#[derive(Clone, Copy, Debug)]
pub struct Sender<'a> {
    /// The nickname of the sender.
    pub nick: &'a str,
    /// The username (ident) of the sender.
    pub username: &'a str,
    /// The hostname of the sender.
    pub hostname: &'a str,
}

impl<'a> Sender<'a> {
    /// Constructs a sender from its parts.
    #[must_use]
    pub const fn new(nick: &'a str, username: &'a str, hostname: &'a str) -> Self {
        Self {
            nick,
            username,
            hostname,
        }
    }

    /// Returns the sender of `message`, or [`None`] for messages without a nickname prefix.
    #[must_use]
    pub fn from_message(message: &'a Message) -> Option<Self> {
        let Some(irc::proto::Prefix::Nickname(nick, username, hostname)) = &message.prefix else {
            return None;
        };

        Some(Sender {
            nick,
            username,
            hostname,
        })
    }
}

/// The IRC command an event was derived from, as an OpenTelemetry messaging operation.
///
/// The messaging conventions record the system-specific operation on every span as
/// `messaging.operation.name`. For IRC that is the command that carried the event, lowercased —
/// `privmsg`, `notice`, `join`, `part`, `quit`, `nick` or `kick`. A modeled command has a static
/// name; an unmodeled command keeps its received name and is lowercased when rendered, so the
/// value stays bounded by the protocol's command vocabulary instead of growing with message
/// content.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessagingOperation<'a> {
    /// A modeled IRC command with a static lowercase name.
    Modeled(&'static str),
    /// An unmodeled command, rendered lowercased.
    Unmodeled(&'a str),
}

impl<'a> MessagingOperation<'a> {
    /// Returns the operation name as a [`Cow`] string.
    ///
    /// A modeled command borrows its static name, and an unmodeled command borrows its received
    /// name when it is already lowercase; only an unmodeled command with uppercase ASCII
    /// allocates the lowercased value. Callers that record the name once per event should call
    /// this instead of formatting the operation for every subscriber.
    #[must_use]
    pub fn into_cow(self) -> Cow<'a, str> {
        match self {
            Self::Modeled(name) => Cow::Borrowed(name),
            Self::Unmodeled(name) => {
                if name.bytes().any(|byte| byte.is_ascii_uppercase()) {
                    Cow::Owned(name.to_ascii_lowercase())
                } else {
                    Cow::Borrowed(name)
                }
            }
        }
    }
}

impl fmt::Display for MessagingOperation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Modeled(name) => f.write_str(name),
            Self::Unmodeled(name) => {
                for character in name.chars() {
                    f.write_char(character.to_ascii_lowercase())?;
                }

                Ok(())
            }
        }
    }
}

/// Generates the sender accessor shared by every event type.
///
/// Declared before its first use so every event struct can invoke it.
macro_rules! event_sender {
    () => {
        /// Returns the sender of the message, if it has a nickname prefix.
        #[must_use]
        pub fn sender(&self) -> Option<Sender<'_>> {
            Sender::from_message(&self.message)
        }

        /// Returns the stable identity of the sender, if it has a nickname prefix.
        ///
        /// The identity prefers the IRCv3 `account` tag — the services account the sender is
        /// logged into, stable across clients and connections — and falls back to the sender's
        /// `user@host`. Plugins that personalize behavior per user resolve their settings
        /// through [`Context::accounts`](crate::Context::accounts) with it, and the
        /// service tracks the hostmasks each identity is seen from.
        #[cfg(feature = "database")]
        #[must_use]
        pub fn identity(&self) -> Option<crate::accounts::UserIdentity> {
            crate::accounts::UserIdentity::from_message(&self.message)
        }
    };
}

/// Builds an event of this type from the IRC message it was derived from.
///
/// Most event kinds are infallible; [`CtcpEvent`] is not, because a `NOTICE` only carries a CTCP
/// message when its text is wrapped in the CTCP delimiter.
trait FromMessage: Sized {
    /// Returns the event, or [`None`] when the message does not carry one.
    fn from_message(message: Arc<Message>) -> Option<Self>;
}

/// Generates a single-field event kind: its struct, constructor, the accessors given in the
/// trailing block, and the shared [`event_sender!`] accessor.
///
/// Every event that carries nothing but the message it was derived from shares this shape;
/// only the accessors differ, so they are passed as a block and stay hand-written with their
/// own documentation and `unreachable!` messages.
macro_rules! event_kind {
    (
        $(#[$event_doc:meta])*
        $event:ident, $ctor:literal,
        { $($accessors:tt)* }
    ) => {
        $(#[$event_doc])*
        #[derive(Clone, Debug)]
        pub struct $event {
            pub(super) message: Arc<Message>,
        }

        impl $event {
            #[must_use]
            #[doc = concat!("Creates a ", $ctor, ".")]
            pub const fn new(message: Arc<Message>) -> Self {
                Self { message }
            }

            $($accessors)*

            event_sender!();
        }

        impl FromMessage for $event {
            fn from_message(message: Arc<Message>) -> Option<Self> {
                Some(Self::new(message))
            }
        }
    };
}

mod command;
mod ctcp;
mod message;
mod presence;
mod raw;
mod subscriptions;
#[cfg(test)]
mod tests;
mod url;

pub use command::CommandEvent;
pub use ctcp::{CtcpEvent, CtcpKind, is_ctcp};
pub use message::MessageEvent;
pub use presence::{JoinEvent, KickEvent, NickEvent, PartEvent, QuitEvent};
pub use raw::RawEvent;
pub use subscriptions::{Event, EventKind, Subscriptions};
pub use url::{UrlEvent, UrlScope};
