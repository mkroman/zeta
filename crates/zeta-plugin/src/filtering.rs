//! Filtering facade for the URL events the host dispatches.
//!
//! The filter service itself lives in the `filter` plugin (in the `plugins` crate). This module
//! is always compiled and exposes the [`Filters`] facade the host's dispatcher consults before
//! delivering a [`UrlEvent`](crate::UrlEvent), so URL events are filtered centrally: with the
//! filter plugin not loaded, it simply never matches.
//!
//! The bridge between the two crates is the [`UrlFilter`] trait: the filter plugin publishes a
//! type-erased [`UrlFilters`] handle of its service to the shared state, and this facade picks
//! it up — the host never needs to name the filter service, and the filter plugin never needs to
//! depend on the host.

use std::sync::Arc;

use url::Url;

use crate::context::Context;
use crate::event::Sender;

/// The object-safe view of a shared URL filter service.
///
/// Implemented by the filter plugin's service; the host consults it through the
/// [`Filters`] facade.
pub trait UrlFilter: Send + Sync {
    /// Whether `url`, posted by `sender` in `channel`, matches any filter and should be ignored.
    fn matches(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool;
}

/// The shared URL filter handle a filter plugin publishes to the shared state.
///
/// A sized wrapper around [`UrlFilter`], so the handle can travel through the type-keyed shared
/// state.
#[derive(Clone)]
pub struct UrlFilters {
    inner: Arc<dyn UrlFilter>,
}

impl UrlFilters {
    /// Wraps a filter service into a publishable handle.
    #[must_use]
    pub fn new(inner: Arc<dyn UrlFilter>) -> Self {
        Self { inner }
    }

    /// Whether `url`, posted by `sender` in `channel`, matches any filter and should be ignored.
    #[must_use]
    pub fn is_filtered(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        self.inner.matches(channel, sender, url)
    }
}

/// A view of the shared filter service.
///
/// Obtained once per message with [`Filters::from_context`]; every URL of the message is then
/// checked with [`Filters::is_filtered`] before its event is delivered.
#[derive(Default)]
pub struct Filters {
    inner: Option<Arc<UrlFilters>>,
}

impl Filters {
    /// Returns the filters published by the filter plugin, if it is loaded.
    #[must_use]
    pub fn from_context(ctx: &Context) -> Self {
        Self {
            inner: ctx.shared.get::<UrlFilters>(),
        }
    }

    /// Whether `url`, posted by `sender` in `channel`, matches any filter and should be ignored.
    #[must_use]
    pub fn is_filtered(&self, channel: &str, sender: Option<Sender<'_>>, url: &Url) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|filters| filters.is_filtered(channel, sender, url))
    }
}
