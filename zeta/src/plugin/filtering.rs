//! Filtering facade for plugins that react to URLs.
//!
//! The filter service itself lives in the feature-gated [`filter`](super::filter) plugin. This
//! module is always compiled and exposes a [`Filters`] facade plus the [`Sender`] description,
//! so URL-handling plugins can consult the shared filters without depending on the
//! `plugin-filter` feature themselves: with the feature disabled, [`Filters`] simply never
//! matches.

use irc::proto::Prefix as IrcPrefix;
use irc::proto::{Command, Message};
use tracing::debug;
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

/// The URLs of a `PRIVMSG` that survive the shared filters.
///
/// Obtained from a message with [`FilteredUrls::from_message`]; iterating yields each extracted
/// URL that no filter matches, in the order they appear in the message.
pub struct FilteredUrls<'a> {
    channel: &'a str,
    filters: Filters,
    sender: Option<Sender<'a>>,
    urls: std::vec::IntoIter<Url>,
}

impl<'a> FilteredUrls<'a> {
    /// Extracts the filtered URLs of a `PRIVMSG`.
    ///
    /// Returns [`None`] for non-`PRIVMSG` messages and messages without any URLs.
    #[must_use]
    pub fn from_message(ctx: &Context, message: &'a Message) -> Option<Self> {
        let Command::PRIVMSG(channel, text) = &message.command else {
            return None;
        };

        let urls = super::extract_urls(text)?;
        let filters = Filters::from_context(ctx);
        let sender = Sender::from_message(message);

        Some(Self {
            channel,
            filters,
            sender,
            urls: urls.into_iter(),
        })
    }

    /// Returns the channel the message was posted in.
    #[must_use]
    pub const fn channel(&self) -> &'a str {
        self.channel
    }
}

impl Iterator for FilteredUrls<'_> {
    type Item = Url;

    fn next(&mut self) -> Option<Self::Item> {
        for url in self.urls.by_ref() {
            if self.filters.is_filtered(self.channel, self.sender, &url) {
                debug!(%url, "skipping filtered url");

                continue;
            }

            return Some(url);
        }

        None
    }
}
