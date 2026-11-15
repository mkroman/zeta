//! Raw events: unmodeled IRC commands forwarded to interested plugins.

use std::sync::Arc;

use irc::proto::{Command, Message};

use super::{FromMessage, Sender};

event_kind! {
    /// An unmodeled IRC command arrived.
    ///
    /// Delivered only to plugins that subscribed with
    /// [`Subscriptions::receive_raw`](super::Subscriptions::receive_raw); numeric
    /// replies and
    /// the connection's own protocol traffic are never delivered to plugins.
    RawEvent, "raw event",
    {
        /// Returns the raw command as received, e.g. `SOMECMD`.
        #[must_use]
        pub fn command(&self) -> &str {
            match &self.message.command {
                Command::Raw(command, _) => command,
                _ => unreachable!("raw events are only delivered for Raw commands"),
            }
        }

        /// Returns the arguments of the raw command.
        #[must_use]
        pub fn args(&self) -> &[String] {
            match &self.message.command {
                Command::Raw(_, args) => args,
                _ => unreachable!("raw events are only delivered for Raw commands"),
            }
        }
    }
}
