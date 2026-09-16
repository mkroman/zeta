//! Filtering facade for plugins that react to URLs.
//!
//! The filter service itself lives in the feature-gated [`filter`](super::filter) plugin. This
//! module is always compiled and exposes a [`Filters`] facade plus the [`Sender`] description,
//! so URL-handling plugins can consult the shared filters without depending on the
//! `plugin-filter` feature themselves: with the feature disabled, [`Filters`] simply never
//! matches.

use irc::proto::Message;
use irc::proto::Prefix as IrcPrefix;
use url::Url;

use super::Context;

/// The sender of a channel message, as identified by its IRC prefix.
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

    /// Returns the sender of `message`, or [`None`] for messages without a nickname prefix
    /// (e.g. server notices), which sender-scoped filters never match.
    #[must_use]
    pub fn from_message(message: &'a Message) -> Option<Sender<'a>> {
        let Some(IrcPrefix::Nickname(nick, username, hostname)) = &message.prefix else {
            return None;
        };

        Some(Sender::new(nick, username, hostname))
    }
}

/// A view of the shared filter service.
///
/// Obtained once per message with [`Filters::from_context`]; every URL of the message is then
/// checked with [`Filters::is_filtered`].
pub struct Filters {
    #[cfg(feature = "plugin-filter")]
    inner: Option<std::sync::Arc<super::filter::FilterService>>,
}

impl Filters {
    /// Returns the filters published by the filter plugin, if it is compiled in and loaded.
    #[must_use]
    pub fn from_context(ctx: &Context) -> Self {
        #[cfg(feature = "plugin-filter")]
        return Self {
            inner: ctx.shared.get::<super::filter::FilterService>(),
        };

        #[cfg(not(feature = "plugin-filter"))]
        {
            let _ = ctx;
            Self {}
        }
    }

    /// Whether `url`, posted by `sender` in `channel`, matches any filter and should be ignored.
    #[must_use]
    pub fn is_filtered(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        #[cfg(feature = "plugin-filter")]
        if let Some(service) = &self.inner {
            return service.matches(channel, sender, url);
        }

        #[cfg(not(feature = "plugin-filter"))]
        let _ = (channel, sender, url);

        false
    }
}
