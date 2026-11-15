//! Channel message events: plain `PRIVMSG` traffic.

use std::sync::Arc;

use irc::proto::Message;

use super::{FromMessage, Sender, privmsg};

event_kind! {
    /// A channel message was posted.
    ///
    /// Delivered to plugins that subscribed with
    /// [`Subscriptions::receive_message`](super::Subscriptions::receive_message); every channel
    /// `PRIVMSG` that is not a CTCP message produces one event, whether or not it matches any
    /// registered command.
    MessageEvent, "message event",
    {
        /// Returns the channel the message was posted in.
        #[must_use]
        pub fn channel(&self) -> &str {
            privmsg(&self.message).0
        }

        /// Returns the text of the message.
        #[must_use]
        pub fn text(&self) -> &str {
            privmsg(&self.message).1
        }
    }
}
