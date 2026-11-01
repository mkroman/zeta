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

use std::sync::{Arc, OnceLock};

use irc::client::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tracing::{Instrument, debug, warn};
use url::Url;
use wreq::redirect::Policy;

use zeta_http as http;
use zeta_http::WreqError;
use zeta_plugin::prelude::*;
use zeta_url::{RedactedUrlRef, last_path_segment, strip_www};
use zeta_utils::irc::unstyle_text;
use zeta_utils::text::Truncatable;

use metadata::{PageMetadata, fetch_metadata};

/// The separator between a page's title and its description.
const CONTENT_SEPARATOR: &str = " — ";

/// File extensions that we avoid requesting to save time and bandwidth.
const BINARY_EXTENSIONS: &[&str] = &[
    ".png", ".jpg", ".bmp", ".gif", ".avi", ".mpg", ".flv", ".3gp", ".mp4", ".exe", ".msi", ".mp3",
    ".flac", ".tar", ".tar.gz", ".tar.bz2", ".zip",
];

/// The maximum number of page fetches that run at once.
///
/// Every fetch streams a response and parks a blocking thread for its HTML tokenizer, so an
/// unbounded number of URLs posted in quick succession would exhaust the blocking pool.
const FETCH_CONCURRENCY: usize = 8;

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
    ///
    /// Shared rather than cloned per URL: posted URLs are frequent and the settings carry the
    /// configured `ignored_hosts`.
    settings: Arc<Settings>,
    /// The catalog of registered plugins, cached on load or on first use.
    ///
    /// The catalog is published once, before any plugin task starts, so the handle stays valid
    /// for the process lifetime. Once a handle is cached, a replacement published later is not
    /// observed.
    catalog: OnceLock<Arc<PluginCatalog>>,
    /// Bounds how many fetches are in flight at once.
    ///
    /// Shared rather than cloned per URL: the permits are the plugin's concurrency budget.
    fetches: Arc<Semaphore>,
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
            settings: Arc::new(settings.clone()),
            catalog: OnceLock::new(),
            fetches: Arc::new(Semaphore::new(FETCH_CONCURRENCY)),
        })
    }

    #[handler(loaded)]
    async fn loaded(&mut self, ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        if let Some(catalog) = ctx.shared.get::<PluginCatalog>() {
            let _ = self.catalog.set(catalog);
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
            debug!(%scheme, url.full = %RedactedUrlRef::from_url(url), "posting repaired url");

            if let Err(error) = client.send_privmsg(event.channel(), plain_reply(url)) {
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

impl Titles {
    /// Returns the catalog of registered plugins.
    ///
    /// Prefers the handle cached by `loaded`, so the per-URL path does not touch shared state.
    /// Shared state is only consulted while nothing is cached — when the load hook has not run
    /// (e.g. in tests calling the handler directly) or when no catalog was published yet — and
    /// the first handle found is cached for the following events.
    fn catalog(&self, ctx: &Context) -> Option<&PluginCatalog> {
        if self.catalog.get().is_none()
            && let Some(catalog) = ctx.shared.get::<PluginCatalog>()
        {
            let _ = self.catalog.set(catalog);
        }

        self.catalog.get().map(Arc::as_ref)
    }

    /// Processes a single extracted URL by spawning a task that fetches the page and posts its
    /// metadata to the channel.
    ///
    /// The caller waits for one of [`FETCH_CONCURRENCY`] permits before the task is spawned, so
    /// a burst of URLs queues in the handler instead of spawning an unbounded number of tasks.
    async fn process_url(&self, url: Arc<Url>, channel: &str, client: &Client) {
        // The permit is acquired before the task exists, so only a bounded number of tasks are
        // alive at once. It is moved into the task and released by its drop, on success, error
        // and cancellation alike.
        let permit = Arc::clone(&self.fetches)
            .acquire_owned()
            .await
            .expect("the fetch semaphore is never closed");

        let http_client = self.client.clone();
        let sender = client.sender();
        let channel = channel.to_string();
        let settings = Arc::clone(&self.settings);

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
            url.full = %RedactedUrlRef::from_url(&url)
        );

        tokio::spawn(
            async move {
                let _permit = permit;

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
    last_path_segment(url).is_some_and(|segment| {
        BINARY_EXTENSIONS
            .iter()
            .any(|extension| ends_with_ignore_ascii_case(segment, extension))
    })
}

/// Whether `haystack` ends with `needle`, ignoring ASCII case and without allocating.
#[must_use]
fn ends_with_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let Some(start) = haystack.len().checked_sub(needle.len()) else {
        return false;
    };

    haystack.as_bytes()[start..].eq_ignore_ascii_case(needle.as_bytes())
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
        .filter(|description| !description.is_empty());

    if title.is_none() && description.is_none() {
        return None;
    }

    // `truncate_within` borrows the description when it already fits, so it is copied into
    // `content` directly instead of through an intermediate, truncated `String`.
    let description = description
        .as_deref()
        .map(|description| description.truncate_within(settings.max_description_length, "…"));

    let title = title.as_deref().unwrap_or_default();
    let description = description.as_deref().unwrap_or_default();

    let separator = (!title.is_empty() && !description.is_empty()).then_some(CONTENT_SEPARATOR);

    let capacity = title.len() + description.len() + separator.map_or(0, str::len);

    let mut content = String::with_capacity(capacity);
    content.push_str(title);

    if let Some(separator) = separator {
        content.push_str(separator);
    }

    content.push_str(description);

    let message = if has_open_graph(metadata) {
        let site = metadata
            .site_name
            .as_deref()
            .map(unstyle_text)
            .filter(|site| !site.is_empty())
            .unwrap_or_else(|| host_name(url).into());

        labeled_reply(&site, &content)
    } else {
        plain_reply(&content)
    };

    // `truncate_within` borrows the message when it already fits, so the byte-length check
    // lets the common short reply skip the `Cow` clone its result would otherwise take.
    if message.len() <= settings.max_message_length {
        return Some(message);
    }

    Some(
        message
            .truncate_within(settings.max_message_length, "…")
            .into_owned(),
    )
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
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;
    use zeta_plugin::CatalogEntry;
    use zeta_test_support::{assert_empty, settings_tests};
    use zeta_utils::irc::strip_formatting;

    /// Starts a minimal HTTP server that answers each request after `delay`, returning its
    /// address, the most connections it ever had open at once, and the number it answered.
    ///
    /// A connection counts as open from the moment it is accepted until its response is started,
    /// so two fetches that overlap show up as two open connections; answering the request is no
    /// longer part of the overlap window.
    async fn delayed_page_server(
        delay: Duration,
    ) -> (SocketAddr, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the test server");
        let address = listener.local_addr().expect("the server has an address");
        let open = Arc::new(AtomicUsize::new(0));
        let most_open = Arc::new(AtomicUsize::new(0));
        let answered = Arc::new(AtomicUsize::new(0));

        tokio::spawn({
            let open = Arc::clone(&open);
            let most_open = Arc::clone(&most_open);
            let answered = Arc::clone(&answered);

            async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let open = Arc::clone(&open);
                    let most_open = Arc::clone(&most_open);
                    let answered = Arc::clone(&answered);

                    tokio::spawn(async move {
                        let now_open = open.fetch_add(1, Ordering::SeqCst) + 1;
                        most_open.fetch_max(now_open, Ordering::SeqCst);

                        tokio::time::sleep(delay).await;

                        let mut request = [0_u8; 1024];
                        let _ = stream.read(&mut request).await;

                        // Leave the overlap window before the response can release the fetch's
                        // permit, so a fetch that starts next never overlaps this one.
                        open.fetch_sub(1, Ordering::SeqCst);

                        let body = "<html><head><title>Hello</title></head></html>";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );

                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.shutdown().await;
                        answered.fetch_add(1, Ordering::SeqCst);
                    });
                }
            }
        });

        (address, most_open, answered)
    }

    /// Returns a catalog whose only entry declares `hosts`.
    fn catalog_with(name: &str, hosts: &'static [&'static str]) -> PluginCatalog {
        PluginCatalog {
            entries: vec![CatalogEntry {
                name: name.to_owned(),
                commands: Vec::new(),
                url_scope: UrlScope::Hosts(hosts),
            }],
        }
    }

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

    #[tokio::test]
    async fn caches_the_catalog_on_load() {
        let ctx = zeta_test_support::test_context();
        ctx.shared.publish(Arc::new(catalog_with(
            "imdb",
            &["imdb.com", "www.imdb.com"],
        )));

        let mut titles = Titles::new(&ctx, &Settings::default()).expect("titles plugin builds");
        let client = Client::from_config(irc::client::data::Config {
            nickname: Some("zeta-test".to_owned()),
            server: Some("mock".to_owned()),
            use_mock_connection: true,
            mock_initial_value: Some(String::new()),
            ..Default::default()
        })
        .await
        .expect("mock irc client");

        titles.loaded(&ctx, &client).await.expect("plugin loads");

        // A catalog published after the load hook does not replace the cached one.
        ctx.shared.publish(Arc::new(catalog_with(
            "youtube",
            &["youtube.com", "youtu.be"],
        )));

        let catalog = titles.catalog(&ctx).expect("catalog is cached");

        assert!(is_catalog_handled(
            &Url::parse("https://www.imdb.com/title/tt1375666").unwrap(),
            Some(catalog)
        ));
        assert!(!is_catalog_handled(
            &Url::parse("https://youtu.be/dQw4w9WgXcQ").unwrap(),
            Some(catalog)
        ));
    }

    #[tokio::test]
    async fn bounds_concurrent_fetches() {
        let ctx = zeta_test_support::test_context();
        let mut titles = Titles::new(&ctx, &Settings::default()).expect("titles plugin builds");

        // One permit: a second fetch may only start once the first one is done.
        titles.fetches = Arc::new(Semaphore::new(1));

        let client = Client::from_config(irc::client::data::Config {
            nickname: Some("zeta-test".to_owned()),
            server: Some("mock".to_owned()),
            use_mock_connection: true,
            mock_initial_value: Some(String::new()),
            ..Default::default()
        })
        .await
        .expect("mock irc client");

        let (address, most_open, answered) = delayed_page_server(Duration::from_millis(300)).await;

        let urls = ["/one", "/two"].map(|path| {
            Arc::new(Url::parse(&format!("http://{address}{path}")).expect("valid url"))
        });

        tokio::join!(
            titles.process_url(Arc::clone(&urls[0]), "#test", &client),
            titles.process_url(Arc::clone(&urls[1]), "#test", &client),
        );

        tokio::time::timeout(Duration::from_secs(5), async {
            while answered.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("both fetches finish");

        assert_eq!(
            most_open.load(Ordering::SeqCst),
            1,
            "more than one fetch was in flight at a time"
        );
    }

    #[tokio::test]
    async fn resolves_an_uncached_catalog_from_shared_state() {
        let ctx = zeta_test_support::test_context();
        ctx.shared.publish(Arc::new(catalog_with(
            "imdb",
            &["imdb.com", "www.imdb.com"],
        )));

        // The load hook never runs when a test calls the handler directly.
        let titles = Titles::new(&ctx, &Settings::default()).expect("titles plugin builds");

        let catalog = titles
            .catalog(&ctx)
            .expect("catalog is resolved from shared state");

        assert!(is_catalog_handled(
            &Url::parse("https://www.imdb.com/title/tt1375666").unwrap(),
            Some(catalog)
        ));

        // The fallback is cached too, so later events do not consult shared state again.
        ctx.shared.publish(Arc::new(catalog_with(
            "youtube",
            &["youtube.com", "youtu.be"],
        )));

        let cached = titles.catalog(&ctx).expect("catalog is cached");

        assert!(!is_catalog_handled(
            &Url::parse("https://youtu.be/dQw4w9WgXcQ").unwrap(),
            Some(cached)
        ));
    }

    #[tokio::test]
    async fn has_no_catalog_until_one_is_published() {
        let ctx = zeta_test_support::test_context();
        let titles = Titles::new(&ctx, &Settings::default()).expect("titles plugin builds");

        assert!(titles.catalog(&ctx).is_none());
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
            &Url::parse("https://maero.dk/photo.Jpg").unwrap()
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
        // Trailing slashes are skipped, so the extension is still found after a double slash.
        assert!(is_binary_url(
            &Url::parse("https://maero.dk/photo.JPG//").unwrap()
        ));

        // Extensionless paths and the root path are pages, not binaries.
        assert!(!is_binary_url(
            &Url::parse("https://maero.dk/page.html").unwrap()
        ));
        assert!(!is_binary_url(&Url::parse("https://maero.dk").unwrap()));
        assert!(!is_binary_url(&Url::parse("https://maero.dk/").unwrap()));
        assert!(!is_binary_url(
            &Url::parse("https://maero.dk/photo").unwrap()
        ));
    }

    #[test]
    fn compares_extension_suffixes_without_allocating() {
        assert!(ends_with_ignore_ascii_case("photo.JPG", ".jpg"));
        assert!(ends_with_ignore_ascii_case("photo.jpg", ".JPG"));
        assert!(ends_with_ignore_ascii_case("archive.tar.gz", ".tar.gz"));
        assert!(!ends_with_ignore_ascii_case("photo.jpeg", ".jpg"));
        // A needle longer than the haystack never matches, and must not panic.
        assert!(!ends_with_ignore_ascii_case("", ".jpg"));
        assert!(!ends_with_ignore_ascii_case(".jp", ".jpg"));
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
