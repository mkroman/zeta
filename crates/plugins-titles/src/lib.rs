//! Posts the title and OpenGraph metadata of every linked page.
//!
//! Monitors messages for HTTP(s) URLs — including the `ttp`/`ttps` variants that are missing
//! their leading `h` — fetches each page, and posts its title and OpenGraph metadata to the
//! channel. Responses are streamed through a push-based HTML tokenizer and the download is
//! aborted as soon as the document head has been received.
//!
//! Response bodies are decoded as UTF-8; byte sequences that are not valid UTF-8 — including
//! entire pages served in another encoding, e.g. ISO-8859-1 — are decoded lossily.

mod metadata;

use irc::client::Client;
use serde::{Deserialize, Serialize};
use tracing::{Instrument, debug, warn};
use url::Url;
use wreq::redirect::Policy;

use zeta_http as http;
use zeta_http::WreqError;
use zeta_plugin::prelude::*;
use zeta_url::{RedactedUrl, path_segments, strip_www};
use zeta_utils::irc::unstyle_text;
use zeta_utils::text::Truncatable;

use metadata::{PageMetadata, fetch_metadata};

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
    /// (see [`Subscriptions::urls`]) are left alone without any configuration, including hosts
    /// of plugins added after this configuration was written.
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
    settings: Settings,
}

#[plugin(name = "titles", settings = Settings, urls(any))]
impl Titles {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client = http::emulated::builder(Some(&ctx.config.http.user_agent))
            .map_err(plugin_err)?
            .redirect(Policy::limited(settings.max_redirects))
            .timeout(ctx.config.http.timeout)
            .build()
            .map_err(|error| plugin_err(WreqError::from(error)))?;

        Ok(Self {
            client,
            settings: settings.clone(),
        })
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
        client: &Client,
        catalog: Option<&PluginCatalog>,
    ) {
        if let Some(scheme) = repaired_from {
            debug!(%scheme, url.full = %RedactedUrl::from_url(&url), "posting repaired url");

            if let Err(error) = client.send_privmsg(channel, plain_reply(&url)) {
                warn!(exception.message = %error, "could not send repaired url");
            }
        }

        if is_ignored_host(&url, &self.settings.ignored_hosts) || is_catalog_handled(&url, catalog)
        {
            debug!(
                url.full = %RedactedUrl::from_url(&url),
                "skipping url handled by another plugin"
            );

            return;
        }

        if is_binary_url(&url) {
            debug!(
                url.full = %RedactedUrl::from_url(&url),
                "skipping url that looks like binary content"
            );

            return;
        }

        let http_client = self.client.clone();
        let sender = client.sender();
        let channel = channel.to_string();
        let settings = self.settings.clone();

        // The span carries the redacted URL: without it, this task's logs would only ever
        // reach stdout. It is a child of the handler span, created while the handler is
        // current, so the fetch stays in the message's trace even though the handler returns
        // (and its span closes) first. The target stays under `zeta`: the span parents the
        // HTTP client span, and disabling it with a plugin log filter would leave that span as
        // a trace root of its own.
        let span = tracing::info_span!(
            target: "zeta.plugins",
            parent: tracing::Span::current(),
            "titles.fetch",
            url.full = %RedactedUrl::from_url(&url)
        );

        tokio::spawn(
            async move {
                let metadata = match fetch_metadata(&http_client, &url).await {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        warn!(
                            exception.message = %error,
                            "could not fetch page metadata"
                        );

                        if let Err(send_error) =
                            sender.send_privmsg(&channel, format!("{REPLY_MARKER} {error}"))
                        {
                            warn!(exception.message = %send_error, "could not send fetch error");
                        }

                        return;
                    }
                };

                if let Some(message) = format_page(&metadata, &url, &settings)
                    && let Err(error) = sender.send_privmsg(&channel, message)
                {
                    warn!(exception.message = %error, "could not send page metadata");
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

/// Whether the host of `url` is configured as ignored.
#[must_use]
fn is_ignored_host(url: &Url, ignored_hosts: &[String]) -> bool {
    url.host_str().is_some_and(|host| {
        ignored_hosts
            .iter()
            .any(|ignored| ignored.eq_ignore_ascii_case(host))
    })
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

    catalog
        .entries
        .iter()
        .any(|plugin| plugin.handles_host(host))
}

/// Whether the path of `url` looks like binary content.
#[must_use]
fn is_binary_url(url: &Url) -> bool {
    path_segments(url)
        .and_then(|segments| segments.last().copied())
        .is_some_and(|segment| {
            let segment = segment.to_ascii_lowercase();
            BINARY_EXTENSIONS
                .iter()
                .any(|extension| segment.ends_with(extension))
        })
}

/// Formats the metadata of a page as an IRC message.
///
/// Returns [`None`] when the page has neither a title nor OpenGraph metadata worth posting.
#[must_use]
fn format_page(metadata: &PageMetadata, url: &Url, settings: &Settings) -> Option<String> {
    let title = metadata
        .title
        .as_deref()
        .or(metadata.og_title.as_deref())
        .map(unstyle_text)
        .filter(|title| !title.is_empty());

    let description = metadata
        .description
        .as_deref()
        .map(unstyle_text)
        .filter(|description| !description.is_empty())
        .map(|description| description.truncate_within(settings.max_description_length, "…"));

    if title.is_none() && description.is_none() {
        return None;
    }

    let content = title
        .as_deref()
        .into_iter()
        .chain(description.as_deref())
        .collect::<Vec<_>>()
        .join(" — ");

    let message = if has_open_graph(metadata) {
        let site = metadata
            .site_name
            .as_deref()
            .map(unstyle_text)
            .filter(|site| !site.is_empty())
            .unwrap_or_else(|| host_name(url));

        labeled_reply(&site, &content)
    } else {
        plain_reply(&content)
    };

    Some(message.truncate_within(settings.max_message_length, "…"))
}

/// Whether any OpenGraph metadata was captured.
#[must_use]
const fn has_open_graph(metadata: &PageMetadata) -> bool {
    metadata.site_name.is_some() || metadata.og_title.is_some() || metadata.description.is_some()
}

/// Returns the host of `url` without its `www.` prefix.
#[must_use]
fn host_name(url: &Url) -> String {
    url.host_str()
        .map_or_default(|host| strip_www(host).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_plugin::CatalogEntry;
    use zeta_test_support::{assert_empty, settings_tests};
    use zeta_utils::irc::strip_formatting;

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
                "WWW.REDDIT.COM".to_string(),
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
        let catalog = PluginCatalog {
            entries: vec![CatalogEntry {
                name: "imdb".to_owned(),
                commands: Vec::new(),
                url_scope: UrlScope::Hosts(&["imdb.com", "www.imdb.com"]),
            }],
        };

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
        assert!(is_binary_url(
            &Url::parse("https://maero.dk/photo.JPG/").unwrap()
        ));

        assert!(!is_binary_url(
            &Url::parse("https://maero.dk/page.html").unwrap()
        ));
        assert!(!is_binary_url(&Url::parse("https://maero.dk").unwrap()));
    }

    #[test]
    fn strips_formatting_from_page_metadata() {
        let metadata = PageMetadata {
            title: Some("\x0310Fake\nTitle\x0f".to_string()),
            site_name: Some("Site\x02".to_string()),
            description: Some("first\nsecond".to_string()),
            ..PageMetadata::default()
        };
        let url = Url::parse("https://maero.dk").unwrap();

        let message =
            strip_formatting(&format_page(&metadata, &url, &Settings::default()).unwrap());

        assert_eq!(message, "> Site: Fake Title — first second");
    }

    #[test]
    fn formats_plain_titles() {
        let metadata = PageMetadata {
            title: Some("Hello  world".to_string()),
            ..PageMetadata::default()
        };
        let url = Url::parse("https://maero.dk").unwrap();

        let message = format_page(&metadata, &url, &Settings::default()).unwrap();

        assert_eq!(message, "\x0310> Hello world");
    }

    #[test]
    fn formats_open_graph_pages() {
        let metadata = PageMetadata {
            title: Some("Hello".to_string()),
            site_name: Some("Maero".to_string()),
            description: Some("A description".to_string()),
            ..PageMetadata::default()
        };
        let url = Url::parse("https://maero.dk").unwrap();

        let message = format_page(&metadata, &url, &Settings::default()).unwrap();

        assert_eq!(
            message,
            "\x0310>\x0f\x02 Maero:\x02\x0310 Hello — A description"
        );
    }

    #[test]
    fn formats_open_graph_fallbacks() {
        let cases = [
            (
                // Without a site name, the host stands in, minus `www.`.
                PageMetadata {
                    og_title: Some("Hello".to_string()),
                    description: Some("A description".to_string()),
                    ..PageMetadata::default()
                },
                "https://www.maero.dk/page",
                "> maero.dk: Hello — A description",
            ),
            (
                // A description-only page still renders as an OpenGraph reply.
                PageMetadata {
                    site_name: Some("Maero".to_string()),
                    description: Some("A description".to_string()),
                    ..PageMetadata::default()
                },
                "https://maero.dk",
                "> Maero: A description",
            ),
        ];

        for (metadata, url, expected) in cases {
            let url = Url::parse(url).unwrap();
            let message = format_page(&metadata, &url, &Settings::default()).unwrap();

            assert_eq!(strip_formatting(&message), expected, "for {url}");
        }
    }

    #[test]
    fn stays_silent_for_pages_without_metadata() {
        let url = Url::parse("https://maero.dk").unwrap();

        assert_eq!(
            format_page(&PageMetadata::default(), &url, &Settings::default()),
            None
        );
        assert_eq!(
            format_page(
                &PageMetadata {
                    title: Some("   ".to_string()),
                    ..PageMetadata::default()
                },
                &url,
                &Settings::default()
            ),
            None
        );
    }

    #[test]
    fn truncates_long_values() {
        let settings = Settings::default();
        let long = "x".repeat(settings.max_message_length + 1);
        let metadata = PageMetadata {
            title: Some(long),
            ..PageMetadata::default()
        };
        let url = Url::parse("https://maero.dk").unwrap();
        let message = format_page(&metadata, &url, &settings).unwrap();

        assert_eq!(message.chars().count(), settings.max_message_length);
        assert!(message.ends_with('…'));
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
