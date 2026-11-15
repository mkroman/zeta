//! URL filtering and page fetch orchestration.
//!
//! Decides which posted URLs the plugin leaves alone — commands and bot replies, configured and
//! catalog-handled hosts, and binary content — and spawns the bounded fetch tasks that post a
//! page's metadata to the channel.

use std::sync::Arc;

use tracing::warn;
use url::Url;

use zeta_plugin::irc::{REPLY_MARKER, plain_reply, send_reply_via};
use zeta_plugin::{Chain, Client, Context, PluginCatalog};

use crate::Titles;
use crate::format::format_page;
use crate::metadata::fetch_metadata;

/// File extensions that we avoid requesting to save time and bandwidth.
const BINARY_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".bmp", ".gif", ".avi", ".mpg", ".flv", ".3gp", ".mp4", ".exe", ".msi", ".mp3",
    ".flac", ".tar", ".tar.gz", ".tar.bz2", ".zip",
];

/// The maximum number of page fetches that run at once.
///
/// Every fetch streams a response and parks a blocking thread for its HTML tokenizer, so an
/// unbounded number of URLs posted in quick succession would exhaust the blocking pool.
pub const FETCH_CONCURRENCY: usize = 8;

impl Titles {
    /// Caches `catalog`.
    ///
    /// The catalog is published once, so the first snapshot wins and a replacement published
    /// later is not observed.
    pub(super) fn cache_catalog(&self, catalog: Arc<PluginCatalog>) {
        let _ = self.catalog.set(catalog);
    }

    /// Returns the catalog of registered plugins.
    ///
    /// Prefers the handle cached by `loaded`, so the per-URL path does not touch shared state.
    /// Shared state is only consulted while nothing is cached — when the load hook has not run
    /// (e.g. in tests calling the handler directly) or when no catalog was published yet — and
    /// the first handle found is cached for the following events.
    pub(super) fn catalog(&self, ctx: &Context) -> Option<&PluginCatalog> {
        if self.catalog.get().is_none()
            && let Some(catalog) = ctx.shared().get::<PluginCatalog>()
        {
            self.cache_catalog(catalog);
        }

        self.catalog.get().map(Arc::as_ref)
    }

    /// Processes a single extracted URL by spawning a task that fetches the page and posts its
    /// metadata to the channel.
    ///
    /// The caller waits for one of [`FETCH_CONCURRENCY`] permits before the task is spawned, so
    /// a burst of URLs queues in the handler instead of spawning an unbounded number of tasks.
    pub(super) async fn process_url(&self, url: Arc<Url>, channel: &str, client: &Client) {
        let http_client = self.client.clone();
        let settings = Arc::clone(&self.settings);

        plugins_shared::spawn_reply_task!(
            Arc::clone(&self.fetches),
            client,
            channel,
            "titles.fetch",
            &url,
            move |sender, channel| async move {
                let metadata = match fetch_metadata(&http_client, &url).await {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        warn!(
                            exception.message = %Chain(&error),
                            "could not fetch page metadata"
                        );

                        if let Err(send_error) =
                            send_reply_via(&sender, &channel, plain_reply(error))
                        {
                            warn!(exception.message = %send_error, "could not send fetch error");
                        }

                        return;
                    }
                };

                if let Some(message) = format_page(&metadata, &url, &settings)
                    && let Err(error) = send_reply_via(&sender, &channel, message)
                {
                    warn!(exception.message = %error, "could not send page metadata");
                }
            },
        )
        .await;
    }
}

/// Whether the message should be ignored: command invocations and messages that look like
/// replies from this or other bots, which could cause feedback loops.
///
/// CTCP messages do not reach the plugin at all — they are their own event kind.
#[must_use]
pub fn should_ignore(text: &str) -> bool {
    // Messages that look like command invocations, e.g. `.gis`.
    if text.starts_with('.')
        && text
            .chars()
            .nth(1)
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return true;
    }

    text.starts_with(REPLY_MARKER)
}

/// Whether the host of `url` is configured as ignored.
#[must_use]
pub fn is_ignored_host(url: &Url, ignored_hosts: &[String]) -> bool {
    url.host_str().is_some_and(|host| {
        ignored_hosts
            .iter()
            .any(|ignored| ignored.eq_ignore_ascii_case(host))
    })
}

/// Whether the host of `url` is declared as handled by a plugin in the catalog.
///
/// Only exact host registrations count: a plugin registered for every URL advertises no specific
/// hosts and is not a reason for titles to skip a link.
#[must_use]
pub fn is_catalog_handled(url: &Url, catalog: Option<&PluginCatalog>) -> bool {
    catalog.is_some_and(|catalog| {
        url.host_str()
            .is_some_and(|host| catalog.handles_host(host))
    })
}

/// Whether the path of `url` looks like binary content.
#[must_use]
pub fn is_binary_url(url: &Url) -> bool {
    zeta_url::last_path_segment(url).is_some_and(|segment| {
        BINARY_EXTENSIONS
            .iter()
            .any(|extension| ends_with_ignore_ascii_case(segment, extension))
    })
}

/// Whether `haystack` ends with `needle`, ignoring ASCII case and without allocating.
#[must_use]
pub fn ends_with_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let Some(start) = haystack.len().checked_sub(needle.len()) else {
        return false;
    };

    haystack.as_bytes()[start..].eq_ignore_ascii_case(needle.as_bytes())
}
