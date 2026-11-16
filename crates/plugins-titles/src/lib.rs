//! Posts the title and OpenGraph metadata of every linked page.
//!
//! Monitors messages for HTTP(s) URLs — including the `ttp`/`ttps` variants that are missing
//! their leading `h` — fetches each page, and posts its title and OpenGraph metadata to the
//! channel. Responses are streamed through a push-based HTML tokenizer and the download is
//! aborted as soon as the document head has been received.
//!
//! Response bodies are decoded as UTF-8; byte sequences that are not valid UTF-8 — including
//! entire pages served in another encoding, e.g. ISO-8859-1 — are decoded lossily.
//!
//! The implementation is split by concern: `fetch` filters the posted URLs and schedules the
//! fetches, `format` renders the reply, and `metadata` extracts the metadata of the document
//! head.

mod fetch;
mod format;
mod metadata;
#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use zeta_http as http;
use zeta_http::WreqError;
use zeta_http::wreq;
use zeta_http::wreq::redirect::Policy;
use zeta_plugin::irc::{plain_reply, send_reply};
use zeta_plugin::{Client, Context, PluginCatalog, UrlEvent, ZetaError, plugin, plugin_err};
use zeta_url::RedactedUrlRef;

use self::fetch::{
    FETCH_CONCURRENCY, is_binary_url, is_catalog_handled, is_ignored_host, should_ignore,
};

/// Settings for the titles plugin, from its `[plugins.titles]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Hosts whose URLs are left to dedicated plugins.
    ///
    /// Empty by default: the hosts that loaded plugins advertise through the plugin catalog
    /// (see [`Subscriptions::urls`](zeta_plugin::Subscriptions::urls)) are left alone without
    /// any configuration, including hosts of plugins added after this configuration was
    /// written.
    pub ignored_hosts: Vec<String>,
    /// The maximum number of redirects to follow.
    pub max_redirects: usize,
    /// The maximum length of a posted message, in characters.
    pub max_message_length: usize,
    /// The maximum length of a posted description, in characters.
    pub max_description_length: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            ignored_hosts: Vec::new(),
            max_redirects: 3,
            max_message_length: 400,
            max_description_length: 200,
        }
    }
}

/// Titles plugin.
///
/// Fetches pages posted as URLs in a channel and posts their title and OpenGraph metadata as
/// soon as the document head has been received.
///
/// Pages are fetched with a client that emulates a modern browser down to its TLS and HTTP/2
/// fingerprints — aggressively bot-protected sites reject plain HTTP clients regardless of the
/// request headers they carry.
pub struct Titles {
    /// The HTTP client used for fetching pages.
    client: wreq::Client,
    /// The plugin settings used when processing URLs.
    ///
    /// Shared rather than cloned per URL: posted URLs are frequent and the settings carry the
    /// configured `ignored_hosts`.
    settings: Arc<Settings>,
    /// The catalog of registered plugins, cached on load or on first use.
    ///
    /// The catalog is published once, before any plugin task starts, so the handle stays valid
    /// for the process lifetime. Once a handle is cached, a replacement published later is not
    /// observed. The catalog indexes the exact hosts that other plugins handle, so the per-URL
    /// check does not walk every entry.
    catalog: OnceLock<Arc<PluginCatalog>>,
    /// Bounds how many fetches are in flight at once.
    ///
    /// Shared rather than cloned per URL: the permits are the plugin's concurrency budget.
    fetches: Arc<Semaphore>,
}

// Manual implementation: the browser-emulated HTTP client is opaque, so it is summarized by
// the settings, the cached catalog size, and the remaining fetch permits.
impl fmt::Debug for Titles {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Titles")
            .field("settings", &self.settings)
            .field(
                "catalog_entries",
                &self
                    .catalog
                    .get()
                    .map_or(0, |catalog| catalog.entries.len()),
            )
            .field("fetches_available", &self.fetches.available_permits())
            .finish_non_exhaustive()
    }
}

#[plugin(name = "titles", settings = Settings, urls(any))]
impl Titles {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client = http::emulated::builder(Some(&ctx.config().http().user_agent))
            .map_err(plugin_err)?
            .redirect(Policy::limited(settings.max_redirects))
            .timeout(ctx.config().http().timeout)
            .build()
            .map_err(|error| plugin_err(WreqError::from(error)))?;

        Ok(Self {
            client,
            settings: Arc::new(settings.clone()),
            catalog: OnceLock::new(),
            fetches: Arc::new(Semaphore::new(FETCH_CONCURRENCY)),
        })
    }

    #[handler(loaded)]
    async fn loaded(&mut self, ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        if let Some(catalog) = ctx.shared().get::<PluginCatalog>() {
            self.cache_catalog(catalog);
        }

        Ok(())
    }

    #[handler(url)]
    async fn handle_url(
        &self,
        ctx: &Context,
        client: &Client,
        event: &UrlEvent,
    ) -> Result<(), ZetaError> {
        if should_ignore(event.text()) {
            return Ok(());
        }

        let url = event.url();

        if let Some(scheme) = event.repaired_from {
            debug!(url.scheme = %scheme, url.full = %RedactedUrlRef::from_url(url), "posting repaired url");

            if let Err(error) = send_reply(client, event.channel(), plain_reply(url)) {
                warn!(exception.message = %error, "could not send repaired url");
            }
        }

        // The checks run on the borrowed URL, before the `Arc` clones needed by the spawned task.
        let catalog = self.catalog(ctx);

        if is_ignored_host(url, &self.settings.ignored_hosts) || is_catalog_handled(url, catalog) {
            debug!(
                url.full = %RedactedUrlRef::from_url(url),
                "skipping url handled by another plugin"
            );

            return Ok(());
        }

        if is_binary_url(url) {
            debug!(
                url.full = %RedactedUrlRef::from_url(url),
                "skipping url that looks like binary content"
            );

            return Ok(());
        }

        self.process_url(event.url_arc(), event.channel(), client)
            .await;

        Ok(())
    }
}
