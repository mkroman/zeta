//! URL events: a URL posted in a channel, scoped by the hosts a plugin handles.

use std::sync::Arc;

use irc::proto::Message;
use url::Url;

use super::{Sender, message_id, privmsg};

/// A URL was posted in a channel.
///
/// The host extracts URLs once per message, applies the shared URL filters, and delivers one
/// event per URL to every plugin that subscribed to its host (or to any host, for generic
/// handlers).
#[derive(Clone, Debug)]
pub struct UrlEvent {
    pub(super) message: Arc<Message>,
    url: Arc<Url>,
    /// The broken scheme prefix as posted, when extraction repaired it (e.g. `ttps`).
    pub repaired_from: Option<&'static str>,
}

impl UrlEvent {
    /// Creates a URL event.
    #[must_use]
    pub const fn new(
        message: Arc<Message>,
        url: Arc<Url>,
        repaired_from: Option<&'static str>,
    ) -> Self {
        Self {
            message,
            url,
            repaired_from,
        }
    }

    /// Returns the URL.
    #[must_use]
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Returns a handle to the URL, sharing the event's allocation.
    ///
    /// Cloning the returned [`Arc`] only bumps its reference count, unlike cloning the [`Url`],
    /// which copies the URL's serialized string. Handlers that move the URL into a spawned task
    /// should use this.
    #[must_use]
    pub fn url_arc(&self) -> Arc<Url> {
        Arc::clone(&self.url)
    }

    /// Returns the channel the URL was posted in.
    #[must_use]
    pub fn channel(&self) -> &str {
        privmsg(&self.message).0
    }

    /// Returns the full text of the message the URL was posted in.
    #[must_use]
    pub fn text(&self) -> &str {
        privmsg(&self.message).1
    }

    /// Returns the IRCv3 `msgid` of the message the URL was posted in, when it carries one.
    #[must_use]
    pub fn message_id(&self) -> Option<&str> {
        message_id(&self.message)
    }

    event_sender!();
}

/// The scope of URLs a plugin registered interest in: which posted URLs it wants to receive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum UrlScope {
    /// The plugin does not handle URLs.
    #[default]
    None,
    /// The plugin handles URLs posted on the given hosts.
    ///
    /// Hosts are matched exactly after ASCII-lowercasing both sides, so list every variant that
    /// can appear in a posted URL (e.g. `imdb.com` as well as `www.imdb.com`).
    Hosts(&'static [&'static str]),
    /// The plugin handles every URL, including hosts other plugins handle.
    ///
    /// Generic handlers receive all URLs and apply their own exclusions (e.g. the hosts
    /// advertised by other plugins through the plugin catalog).
    Any,
}
