//! Presence events: joins, parts, quits, nickname changes and kicks.

use std::sync::Arc;

use irc::proto::{Command, Message};

use super::{FromMessage, Sender};

event_kind! {
    /// A user joined a channel.
    JoinEvent, "join event",
    {
        /// Returns the channel that was joined.
        #[must_use]
        pub fn channel(&self) -> &str {
            match &self.message.command {
                Command::JOIN(channel, ..) => channel,
                _ => unreachable!("join events are only delivered for JOIN messages"),
            }
        }

        /// Returns the nickname of the user that joined.
        #[must_use]
        pub fn nick(&self) -> &str {
            Sender::from_message(&self.message).map_or("", |sender| sender.nick)
        }
    }
}

event_kind! {
    /// A user left a channel.
    PartEvent, "part event",
    {
        /// Returns the channel that was left.
        #[must_use]
        pub fn channel(&self) -> &str {
            match &self.message.command {
                Command::PART(channel, ..) => channel,
                _ => unreachable!("part events are only delivered for PART messages"),
            }
        }

        /// Returns the parting comment, if any.
        #[must_use]
        pub fn reason(&self) -> Option<&str> {
            match &self.message.command {
                Command::PART(_, reason) => reason.as_deref(),
                _ => unreachable!("part events are only delivered for PART messages"),
            }
        }
    }
}

event_kind! {
    /// A user quit the network.
    QuitEvent, "quit event",
    {
        /// Returns the quit message, if any.
        #[must_use]
        pub fn reason(&self) -> Option<&str> {
            match &self.message.command {
                Command::QUIT(reason) => reason.as_deref(),
                _ => unreachable!("quit events are only delivered for QUIT messages"),
            }
        }
    }
}

event_kind! {
    /// A user changed their nickname; the previous identity is available through
    /// [`sender`](Self::sender).
    NickEvent, "nick change event",
    {
        /// Returns the new nickname.
        #[must_use]
        pub fn new_nick(&self) -> &str {
            match &self.message.command {
                Command::NICK(nick) => nick,
                _ => unreachable!("nick events are only delivered for NICK messages"),
            }
        }
    }
}

event_kind! {
    /// A user was kicked from a channel.
    KickEvent, "kick event",
    {
        /// Returns the channel the user was kicked from.
        #[must_use]
        pub fn channel(&self) -> &str {
            match &self.message.command {
                Command::KICK(channel, ..) => channel,
                _ => unreachable!("kick events are only delivered for KICK messages"),
            }
        }

        /// Returns the nickname of the user that was kicked.
        #[must_use]
        pub fn target(&self) -> &str {
            match &self.message.command {
                Command::KICK(_, target, _) => target,
                _ => unreachable!("kick events are only delivered for KICK messages"),
            }
        }

        /// Returns the kick reason, if any.
        #[must_use]
        pub fn reason(&self) -> Option<&str> {
            match &self.message.command {
                Command::KICK(.., reason) => reason.as_deref(),
                _ => unreachable!("kick events are only delivered for KICK messages"),
            }
        }
    }
}
