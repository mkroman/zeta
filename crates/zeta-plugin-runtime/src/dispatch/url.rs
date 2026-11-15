//! The URLs retained from one dispatched line, resolved before the message is shared.

use std::sync::Arc;

use url::Url;

use super::subscriber::Subscriber;

/// A URL retained for dispatch: its shared handle, the broken scheme prefix it was repaired
/// from, and the host subscribers it must reach, resolved before the URL was shared.
pub(super) struct UrlDispatch<'a> {
    /// The shared URL handle.
    pub(super) url: Arc<Url>,
    /// The broken scheme prefix as posted, when extraction repaired it (e.g. `ttps`).
    pub(super) repaired_from: Option<&'static str>,
    /// The host subscribers the URL must reach.
    pub(super) subscribers: &'a [Subscriber],
}
