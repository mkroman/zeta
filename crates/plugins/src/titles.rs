//! Posts the title and OpenGraph metadata of every linked page.
//!
//! Monitors messages for HTTP(s) URLs — including the `ttp`/`ttps` variants that are missing
//! their leading `h` — fetches each page, and posts its title and OpenGraph metadata to the
//! channel. Responses are streamed through a push-based HTML tokenizer and the download is
//! aborted as soon as the document head has been received.
//!
//! Response bodies are decoded as UTF-8; byte sequences that are not valid UTF-8 — including
//! entire pages served in another encoding, e.g. ISO-8859-1 — are decoded lossily.

mod fetch;
mod metadata;
mod reply;

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tracing::{Instrument, debug, warn};
use url::Url;
use wreq::redirect::Policy;

use self::fetch::fetch_metadata;
use self::reply::format_page;
use crate::prelude::*;
use zeta_http as http;
use zeta_http::WreqError;
use zeta_url::redact_url;

/// File extensions that we avoid requesting to save time and bandwidth.
const BINARY_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".bmp", ".gif", ".avi", ".mpg", ".flv", ".3gp", ".mp4", ".exe", ".msi", ".mp3",
    ".flac", ".tar", ".tar.gz", ".tar.bz2", ".zip",
];

/// Settings for the titles plugin, from its `[plugins.titles]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Hosts whose URLs are left to dedicated plugins.
    ///
    /// Empty by default: the hosts that loaded plugins advertise through the plugin catalog
    /// (see [`Subscriptions::subscribe_urls`]) are left alone without any configuration,
    /// including hosts of plugins added after this configuration was written.
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
    /// The plugin settings used when processing URLs, shared with the fetch tasks.
    settings: Arc<Settings>,
}

#[async_trait]
impl Plugin<Context> for Titles {
    type Settings = Settings;

    fn new(
        ctx: &Context,
        settings: &Settings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, PluginError> {
        let client = http::emulated::builder(Some(&ctx.config.http.user_agent))?
            .redirect(Policy::limited(settings.max_redirects))
            .timeout(ctx.config.http.timeout)
            .build()
            .map_err(|error| plugin_err(WreqError::from(error)))?;

        subscriptions.subscribe_urls(UrlScope::Any);

        Ok(Self {
            client,
            settings: Arc::new(settings.clone()),
        })
    }

    async fn handle_url(
        &self,
        ctx: &Context,
        client: &IrcClient,
        event: &UrlEvent,
    ) -> Result<(), PluginError> {
        if should_ignore(event.text()) {
            return Ok(());
        }

        let catalog = ctx.shared.get::<PluginCatalog>();
        self.process_url(
            event.url().clone(),
            event.repaired_from,
            event.channel(),
            client,
            catalog.as_deref(),
        );

        Ok(())
    }
}

impl Titles {
    /// Processes a single extracted URL: announces repaired schemes, and spawns a task that
    /// fetches the page and posts its metadata to the channel.
    fn process_url(
        &self,
        url: Url,
        repaired_from: Option<&'static str>,
        channel: &str,
        client: &IrcClient,
        catalog: Option<&PluginCatalog>,
    ) {
        if let Some(scheme) = repaired_from {
            debug!(%scheme, %url, "posting repaired url");

            if let Err(error) = client.send_privmsg(channel, plain_reply(&url)) {
                warn!(%error, "could not send repaired url");
            }
        }

        if is_ignored_host(&url, &self.settings.ignored_hosts) || is_catalog_handled(&url, catalog)
        {
            debug!(%url, "skipping url handled by another plugin");

            return;
        }

        if is_binary_url(&url) {
            debug!(%url, "skipping url that looks like binary content");

            return;
        }

        let http_client = self.client.clone();
        let sender = client.sender();
        let channel = channel.to_string();
        let settings = Arc::clone(&self.settings);

        // The span carries the redacted URL: without it, this task's logs would only ever
        // reach stdout.
        let span = tracing::info_span!("fetch_page_metadata", url.full = %redact_url(&url));

        tokio::spawn(
            async move {
                let metadata = match fetch_metadata(&http_client, &url).await {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        warn!(
                            url.full = %redact_url(&url),
                            %error,
                            "could not fetch page metadata"
                        );

                        if let Err(send_error) =
                            sender.send_privmsg(&channel, format!("{REPLY_MARKER} {error}"))
                        {
                            warn!(%send_error, "could not send fetch error");
                        }

                        return;
                    }
                };

                if let Some(message) = format_page(&metadata, &url, &settings)
                    && let Err(error) = sender.send_privmsg(&channel, message)
                {
                    warn!(%error, "could not send page metadata");
                }
            }
            .instrument(span),
        );
    }
}

/// Whether the message should be ignored: command invocations and messages that look like
/// replies from this or other bots, which could cause feedback loops.
///
/// CTCP messages do not reach the plugin at all — they are their own event kind.
#[must_use]
fn should_ignore(text: &str) -> bool {
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

/// Whether the host of `url` is handled by another plugin.
#[must_use]
fn is_ignored_host(url: &Url, ignored_hosts: &[String]) -> bool {
    url.host_str()
        .is_some_and(|host| ignored_hosts.iter().any(|ignored| ignored == host))
}

/// Whether the host of `url` is declared as handled by a plugin in the catalog.
///
/// Hosts are compared exactly against the hosts advertised through the plugin catalog; both
/// sides are already lowercase.
#[must_use]
fn is_catalog_handled(url: &Url, catalog: Option<&PluginCatalog>) -> bool {
    let Some(catalog) = catalog else {
        return false;
    };

    let Some(host) = url.host_str() else {
        return false;
    };

    catalog.handles_url_host(host)
}

/// Whether the path of `url` looks like binary content.
#[must_use]
fn is_binary_url(url: &Url) -> bool {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .is_some_and(|segment| {
            let segment = segment.to_ascii_lowercase();
            BINARY_EXTENSIONS
                .iter()
                .any(|extension| segment.ends_with(extension))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_plugin::PluginEntry;
    use zeta_test_support::{assert_empty, settings_tests};

    #[test]
    fn ignores_command_messages() {
        assert!(should_ignore(".gis https://maero.dk"));
        assert!(should_ignore(".ofn"));
        assert!(should_ignore(".help"));

        assert!(!should_ignore("a .dot in a sentence https://maero.dk"));
        assert!(!should_ignore("... https://maero.dk"));
    }

    #[test]
    fn ignores_bot_replies() {
        // CTCP messages never reach the plugin at all: they are their own event kind.
        assert!(should_ignore("\x0310> Some page"));
        assert!(should_ignore("\x0310>\x0f\x02 Site:\x02\x0310 Some page"));

        assert!(!should_ignore("check this out https://maero.dk"));
    }

    #[test]
    fn ignores_configured_hosts() {
        let settings = Settings {
            ignored_hosts: vec![
                "www.reddit.com".to_string(),
                "youtu.be".to_string(),
                "vm.tiktok.com".to_string(),
            ],
            ..Settings::default()
        };

        assert!(is_ignored_host(
            &Url::parse("https://www.reddit.com/r/rust").unwrap(),
            &settings.ignored_hosts
        ));
        assert!(is_ignored_host(
            &Url::parse("https://youtu.be/abc").unwrap(),
            &settings.ignored_hosts
        ));
        assert!(is_ignored_host(
            &Url::parse("https://vm.tiktok.com/abc").unwrap(),
            &settings.ignored_hosts
        ));

        assert!(!is_ignored_host(
            &Url::parse("https://maero.dk").unwrap(),
            &settings.ignored_hosts
        ));
    }

    #[test]
    fn ignores_hosts_declared_in_the_catalog() {
        let catalog = PluginCatalog::new(vec![PluginEntry {
            name: "imdb".into(),
            commands: Vec::new(),
            url_hosts: vec!["imdb.com", "www.imdb.com"],
        }]);

        assert!(is_catalog_handled(
            &Url::parse("https://www.imdb.com/title/tt1375666").unwrap(),
            Some(&catalog)
        ));
        assert!(is_catalog_handled(
            &Url::parse("https://IMDB.com/title/tt1375666").unwrap(),
            Some(&catalog)
        ));
        assert!(!is_catalog_handled(
            &Url::parse("https://maero.dk").unwrap(),
            Some(&catalog)
        ));
        assert!(!is_catalog_handled(
            &Url::parse("https://maero.dk").unwrap(),
            None
        ));
    }

    #[test]
    fn ignores_binary_urls() {
        assert!(is_binary_url(
            &Url::parse("https://maero.dk/photo.JPG").unwrap()
        ));
        assert!(is_binary_url(
            &Url::parse("https://maero.dk/archive.tar.gz").unwrap()
        ));
        assert!(is_binary_url(
            &Url::parse("https://maero.dk/video.mp4?start=30").unwrap()
        ));

        assert!(!is_binary_url(
            &Url::parse("https://maero.dk/page.html").unwrap()
        ));
        assert!(!is_binary_url(&Url::parse("https://maero.dk").unwrap()));
    }

    settings_tests! {
        Settings,
        settings,
        default: {
            assert_empty!(settings.ignored_hosts);
            assert_eq!(settings.max_redirects, 3);
            assert_eq!(settings.max_message_length, 400);
            assert_eq!(settings.max_description_length, 200);
        }
        deserialize: {
            "ignored_hosts": ["example.com"],
            "max_redirects": 1,
            "max_message_length": 100,
            "max_description_length": 50,
        } assert: {
            assert_eq!(settings.ignored_hosts, ["example.com"]);
            assert_eq!(settings.max_redirects, 1);
            assert_eq!(settings.max_message_length, 100);
            assert_eq!(settings.max_description_length, 50);
        }
    }
}
