//! Filtering facade for the events the host dispatches.
//!
//! The filter service itself lives in the `filter` plugin (in the `plugins` crate). This module
//! is always compiled and exposes the [`Filters`] handle the host's dispatcher consults before
//! delivering events, so filtered URL events are dropped centrally and messages from ignored
//! senders never reach the plugins: with the filter plugin not loaded, nothing ever matches.
//!
//! The bridge between the two crates is the [`UrlFilter`] trait: the filter plugin publishes a
//! type-erased [`Filters`] handle of its service to the shared state, and this facade picks it
//! up — the host never needs to name the filter service, and the filter plugin never needs to
//! depend on the host.

use std::sync::Arc;

use url::Url;

use crate::context::Context;
use crate::event::Sender;

/// The object-safe view of a shared URL filter service.
///
/// Implemented by the filter plugin's service; the host consults it through the
/// [`Filters`] handle.
pub trait UrlFilter: Send + Sync {
    /// Whether `url`, posted by `sender` in `channel`, matches any filter and should be ignored.
    fn is_filtered(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool;

    /// Whether messages from `sender` in `channel` are ignored entirely.
    ///
    /// Backed by filters without URL criteria (host and path unset): a matching sender's
    /// messages are not delivered to the message subscribers, their commands do not trigger,
    /// and — through [`UrlFilter::is_filtered`] — their URLs are filtered too.
    fn ignores(&self, channel: &str, sender: Option<Sender<'_>>) -> bool;
}

/// The shared filter handle the filter plugin publishes to the shared state.
///
/// A cloneable, type-erased wrapper around [`UrlFilter`], so the handle can travel through the
/// type-keyed shared state. With no filter plugin loaded, the handle is empty and matches
/// nothing.
#[derive(Clone, Default)]
pub struct Filters {
    inner: Option<Arc<dyn UrlFilter>>,
}

impl Filters {
    /// Wraps a filter service into a publishable handle.
    #[must_use]
    pub fn new(inner: Arc<dyn UrlFilter>) -> Self {
        Self { inner: Some(inner) }
    }

    /// Returns the filters published by the filter plugin, or an empty handle when it is not
    /// loaded.
    #[must_use]
    pub fn from_context(ctx: &Context) -> Self {
        ctx.shared
            .get::<Self>()
            .map_or_else(Self::default, |filters| (*filters).clone())
    }

    /// Whether `url`, posted by `sender` in `channel`, matches any filter and should be ignored.
    #[must_use]
    pub fn is_filtered(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|filters| filters.is_filtered(channel, sender, url))
    }

    /// Whether messages from `sender` in `channel` are ignored entirely.
    #[must_use]
    pub fn ignores(&self, channel: &str, sender: Option<Sender<'_>>) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|filters| filters.ignores(channel, sender))
    }
}
