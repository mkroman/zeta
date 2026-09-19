//! Filtering facade for the URL events the host dispatches.
//!
//! The filter service itself lives in the feature-gated [`filter`](crate::plugin::filter)
//! plugin. This module is always compiled and exposes the
//! [`Filters`](crate::plugin::filtering::Filters) facade the host's dispatcher consults before
//! delivering a [`UrlEvent`](zeta_plugin::UrlEvent), so URL events are filtered centrally: with
//! the feature disabled, it simply never matches.

use url::Url;
use zeta_plugin::event::Sender;

use super::Context;

/// A view of the shared filter service.
///
/// Obtained once per message with [`Filters::from_context`]; every URL of the message is then
/// checked with [`Filters::is_filtered`] before its event is delivered.
#[derive(Default)]
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
            Self::default()
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
