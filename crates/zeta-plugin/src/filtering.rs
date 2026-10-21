//! Filtering facade for the events the host dispatches.
//!
//! The filter service itself lives in the `filter` plugin (in the `plugins` crate). This module
//! is always compiled and exposes the [`FilterGate`] facade the host's dispatcher consults before
//! delivering events, so filtered URL events are dropped centrally and messages from ignored
//! senders never reach the plugins: with the filter plugin not loaded, nothing ever matches.
//!
//! The bridge between the two crates is the [`FilterPolicy`] trait: the filter plugin publishes a
//! type-erased [`FilterHandle`] of its service to the shared state, and this facade picks
//! it up — the host never needs to name the filter service, and the filter plugin never needs to
//! depend on the host.

use std::sync::Arc;

use url::Url;

use crate::context::Context;
use crate::event::Sender;

/// The object-safe view of a shared URL filter service.
///
/// Implemented by the filter plugin's service; the host consults it through the
/// [`FilterGate`] facade.
pub trait FilterPolicy: Send + Sync {
    /// Whether `url`, posted by `sender` in `channel`, matches any filter and should be blocked.
    fn blocks_url(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool;

    /// Whether messages from `sender` in `channel` are ignored entirely.
    ///
    /// Backed by filters without URL criteria (host and path unset): a matching sender's
    /// messages are not delivered to the message subscribers, their commands do not trigger,
    /// and — through [`FilterPolicy::blocks_url`] — their URLs are filtered too.
    fn ignores_sender(&self, channel: &str, sender: Option<Sender<'_>>) -> bool;
}

/// The shared URL filter handle a filter plugin publishes to the shared state.
///
/// A sized wrapper around [`FilterPolicy`], so the handle can travel through the type-keyed shared
/// state.
#[derive(Clone)]
pub struct FilterHandle {
    inner: Arc<dyn FilterPolicy>,
}

impl FilterHandle {
    /// Wraps a filter service into a publishable handle.
    #[must_use]
    pub fn new(inner: Arc<dyn FilterPolicy>) -> Self {
        Self { inner }
    }

    /// Whether `url`, posted by `sender` in `channel`, matches any filter and should be blocked.
    #[must_use]
    pub fn blocks_url(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        self.inner.blocks_url(channel, sender, url)
    }

    /// Whether messages from `sender` in `channel` are ignored entirely.
    #[must_use]
    pub fn ignores_sender(&self, channel: &str, sender: Option<Sender<'_>>) -> bool {
        self.inner.ignores_sender(channel, sender)
    }
}

/// A view of the shared filter service.
///
/// Obtained once per message with [`FilterGate::from_context`]; the sender is checked with
/// [`FilterGate::ignores_sender`] before any event is delivered, and every URL of the message
/// with [`FilterGate::blocks_url`] before its event is delivered.
#[derive(Default)]
pub struct FilterGate {
    inner: Option<Arc<FilterHandle>>,
}

impl FilterGate {
    /// Returns the filter policy published by the filter plugin, if it is loaded.
    #[must_use]
    pub fn from_context(ctx: &Context) -> Self {
        Self {
            inner: ctx.shared.get::<FilterHandle>(),
        }
    }

    /// Whether `url`, posted by `sender` in `channel`, matches any filter and should be blocked.
    #[must_use]
    pub fn blocks_url(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|filters| filters.blocks_url(channel, sender, url))
    }

    /// Whether messages from `sender` in `channel` are ignored entirely.
    #[must_use]
    pub fn ignores_sender(&self, channel: &str, sender: Option<Sender<'_>>) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|filters| filters.ignores_sender(channel, sender))
    }
}
