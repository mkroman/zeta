//! Instagram integration.
//!
//! Summarises Instagram media links — feed posts, reels, and IGTV videos — using the OpenGraph
//! metadata of the linked page, and, if mirroring is configured, downloads the videos with
//! `yt-dlp` and mirrors them to an S3-compatible bucket, replying with a public link to the
//! mirrored file. Stories are mirrored without a summary: their pages carry no metadata without
//! an authenticated session.
//!
//! Pages are fetched with a client that emulates a modern browser down to its TLS and HTTP/2
//! fingerprints; Instagram serves a login wall to requests it scores as bot traffic, in which case
//! the link is only mirrored, without a summary.
//!
//! Mirroring is configured through the top-level `[mirror]` configuration section (or the `S3_*`
//! environment variables); without it, the plugin only posts summaries.

mod meta;
mod urls;

use std::fmt::Write;

use serde::{Deserialize, Serialize};
use tracing::{debug, error};
use url::Url;
use wreq::redirect::Policy;
use wreq_util::Emulation;

use crate::{
    mirror::{Mirror, MirrorTarget},
    plugin::prelude::*,
    utils::Truncatable,
};

use self::meta::PageMetadata;
use self::urls::{InstagramLink, MediaKind, media_url, parse_instagram_url, story_url};

/// The default public URL that mirrored media are linked with.
const DEFAULT_PUBLIC_URL_BASE: &str = "https://pub.rwx.im/instagram";

/// Settings for the instagram plugin, from its `[plugins.instagram]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Settings {
    /// The maximum length of a media caption before it gets truncated.
    #[serde(default = "default_title_length")]
    pub title_length: usize,
    /// The key prefix that mirrored media are uploaded under.
    ///
    /// Falls back to the `INSTAGRAM_S3_PREFIX` environment variable, and to `instagram` when
    /// neither is set.
    #[serde(default)]
    pub prefix: Option<String>,
    /// The base URL used when linking to mirrored media.
    ///
    /// Falls back to the `INSTAGRAM_PUBLIC_URL_BASE` environment variable when unset. Links are
    /// built by appending the media id as a URL fragment, so the base must point at a viewer page
    /// that resolves the fragment — not directly at the bucket.
    #[serde(default)]
    pub public_url_base: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            title_length: default_title_length(),
            prefix: None,
            public_url_base: None,
        }
    }
}

/// Returns the default maximum caption length.
const fn default_title_length() -> usize {
    150
}

pub struct Instagram {
    /// The HTTP client used for fetching pages, emulating a modern browser.
    client: wreq::Client,
    /// The shared mirror handle, when mirroring is configured.
    mirror: Option<MirrorTarget>,
    /// The plugin settings used when processing URLs.
    settings: Settings,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] wreq::Error),
    #[error("share link did not resolve to a valid url")]
    InvalidRedirect,
}

#[async_trait]
impl Plugin<Context> for Instagram {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Instagram, ZetaError> {
        subscriptions.urls(UrlScope::Hosts(urls::URL_HOSTS));

        let client = wreq::Client::builder()
            .emulation(Emulation::Firefox142)
            .redirect(Policy::limited(4))
            .timeout(ctx.config.http.timeout)
            .build()
            .map_err(plugin_err)?;

        let mirror = MirrorTarget::resolve(
            ctx.shared.get::<Mirror>(),
            settings.prefix.as_deref(),
            "INSTAGRAM_S3_PREFIX",
            "instagram",
            settings.public_url_base.as_deref(),
            "INSTAGRAM_PUBLIC_URL_BASE",
            DEFAULT_PUBLIC_URL_BASE,
        );

        Ok(Instagram {
            client,
            mirror,
            settings: settings.clone(),
        })
    }

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        if let Some(mirror) = &self.mirror {
            mirror.start_downloads();
        }

        Ok(())
    }

    async fn handle_url(&self, _ctx: &Context, client: &Client, url: &UrlEvent) -> Result<(), ZetaError> {
        if let Err(err) = self
            .process_urls(&[url.url().clone()], url.channel(), client)
            .await
        {
            error!("could not process urls: {err}");
        }

        Ok(())
    }
}

impl Instagram {
    async fn process_urls(
        &self,
        urls: &[Url],
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        for url in urls {
            self.process_url(url, channel, client).await?;
        }

        Ok(())
    }

    async fn process_url(&self, url: &Url, channel: &str, client: &Client) -> Result<(), Error> {
        match parse_instagram_url(url) {
            Some(InstagramLink::Media { kind, id }) => {
                debug!(%id, "processing media");

                self.process_media(&kind, &id, channel, client).await?;
            }
            Some(InstagramLink::Story { username, id }) => {
                debug!(%id, %username, "processing story");

                self.process_story(&username, &id, channel, client).await?;
            }
            Some(InstagramLink::Shortened(url)) => {
                debug!(%url, "resolving share link");

                let resolved_url = self.resolve_url(&url).await?;

                if let Some(InstagramLink::Media { kind, id }) = parse_instagram_url(&resolved_url)
                {
                    self.process_media(&kind, &id, channel, client).await?;
                }
            }
            // Profile links are only claimed so that they are not announced by the titles plugin.
            Some(InstagramLink::Profile(_)) | None => {}
        }

        Ok(())
    }

    async fn process_media(
        &self,
        kind: &MediaKind,
        id: &str,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        let url = media_url(*kind, id);
        let metadata = self.fetch_metadata(&url).await;

        if let Some(summary) = metadata
            .as_ref()
            .and_then(|meta| format_summary(meta, kind.label(), self.settings.title_length))
        {
            let _ = client.send_privmsg(channel, notice(&summary));
        }

        self.mirror_video(&url, id, channel, client).await;

        Ok(())
    }

    async fn process_story(
        &self,
        username: &str,
        id: &str,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        let url = story_url(username, id);

        if let Some(summary) = self
            .fetch_metadata(&url)
            .await
            .as_ref()
            .and_then(|meta| format_summary(meta, "story", self.settings.title_length))
        {
            let _ = client.send_privmsg(channel, notice(&summary));
        }

        self.mirror_video(&url, id, channel, client).await;

        Ok(())
    }

    /// Fetches the OpenGraph metadata of the given page.
    ///
    /// Returns `None` when the page could not be fetched, or is a login wall carrying no metadata.
    async fn fetch_metadata(&self, url: &str) -> Option<PageMetadata> {
        match meta::fetch(&self.client, url).await {
            Ok(metadata) => Some(metadata),
            Err(error) => {
                debug!(%url, %error, "could not fetch page metadata");

                None
            }
        }
    }

    /// Requests the given share link and returns the URL it resolves to.
    async fn resolve_url(&self, url: &Url) -> Result<Url, Error> {
        debug!(%url, "resolving share url");

        let response = self.client.get(url.as_str()).send().await?;

        // The final URI, after the followed redirects, identifies the shared resource.
        let location = response.uri().to_string();
        let url = Url::parse(&location).map_err(|_| Error::InvalidRedirect)?;

        debug!(%url, "resolved share url");

        Ok(url)
    }

    /// Starts mirroring of the given media, replying with a link to the mirrored file.
    ///
    /// If the media has already been mirrored, the existing link is sent immediately; otherwise
    /// the download and upload happens in a background task that replies with the link.
    async fn mirror_video(&self, url: &str, id: &str, channel: &str, client: &Client) {
        let Some(mirror) = &self.mirror else {
            return;
        };

        let sender = client.sender();

        let on_mirrored = {
            let channel = channel.to_string();
            move |link: String| {
                let _ = sender.send_privmsg(&channel, notice(&link));
            }
        };

        match mirror.ensure_mirrored(url, id, on_mirrored).await {
            Ok(Some(link)) => {
                let _ = client.send_privmsg(channel, notice(&link));
            }
            Ok(None) => {}
            Err(err) => {
                error!(%id, error = %err, "could not check if the media is already mirrored");
            }
        }
    }
}

/// Formats the OpenGraph metadata as a human-readable summary of the media.
///
/// The `og:title` of a media page has the form `<author> on Instagram: "<caption>"`; when the
/// title is generic — as it is on login walls — no summary is posted.
fn format_summary(meta: &PageMetadata, kind: &str, title_length: usize) -> Option<String> {
    let title = meta
        .og_title
        .as_deref()
        .map(str::trim)
        .filter(|title| !is_generic_title(title))?;

    let (author, caption) = split_author_title(title);
    let caption = [caption, meta.og_description.as_deref().unwrap_or_default()]
        .into_iter()
        .map(str::trim)
        .find(|caption| !caption.is_empty());

    let mut buf = String::new();

    if let Some(caption) = caption {
        let truncated = caption.truncate_with_suffix(title_length, "…");
        let _ = write!(buf, "“\x0f{}\x0310” ", truncated.trim());
    }

    if let Some(author) = author {
        let _ = write!(buf, "is an Instagram {kind} by\x0f {author}");
    } else if !buf.is_empty() {
        let _ = write!(buf, "is an Instagram {kind}");
    }

    (!buf.is_empty()).then_some(buf)
}

/// Returns whether the title is the generic title of a login or placeholder page.
const fn is_generic_title(title: &str) -> bool {
    title.eq_ignore_ascii_case("Instagram") || title.eq_ignore_ascii_case("Login • Instagram")
}

/// Splits an `og:title` of the form `<author> on Instagram: "<caption>"` into its author and
/// caption, trimming the quotes wrapping the caption.
#[must_use]
fn split_author_title(title: &str) -> (Option<&str>, &str) {
    match title.split_once(" on Instagram: ") {
        Some((author, caption)) => (Some(author.trim()), caption.trim_matches('"').trim()),
        None => (None, title),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert_eq!(settings.title_length, 150);
            assert!(settings.prefix.is_none());
            assert!(settings.public_url_base.is_none());
        }
        deserialize: {
            "title_length": 100,
            "prefix": "~meta/instagram",
            "public_url_base": "https://pub.example.com/instagram",
        } assert: {
            assert_eq!(settings.title_length, 100);
            assert_eq!(settings.prefix.as_deref(), Some("~meta/instagram"));
            assert_eq!(
                settings.public_url_base.as_deref(),
                Some("https://pub.example.com/instagram")
            );
        }
    }

    /// The `og:title` of an Instagram media page.
    const MEDIA_TITLE: &str = "user.name on Instagram: \"Some caption & more\"";

    /// A generic `og:title`, as found on login walls.
    const GENERIC_TITLE: &str = "Login • Instagram";

    #[test]
    fn test_split_author_title() {
        let (author, caption) = split_author_title(MEDIA_TITLE);

        assert_eq!(author, Some("user.name"));
        assert_eq!(caption, "Some caption & more");

        let (author, caption) = split_author_title("Just a title");

        assert_eq!(author, None);
        assert_eq!(caption, "Just a title");

        // Empty captions stay empty; the summary falls back to the description.
        let (author, caption) = split_author_title("user.name on Instagram: \"\"");

        assert_eq!(author, Some("user.name"));
        assert_eq!(caption, "");
    }

    #[test]
    fn test_is_generic_title() {
        assert!(is_generic_title("Instagram"));
        assert!(is_generic_title(GENERIC_TITLE));
        assert!(!is_generic_title("user.name on Instagram: \"caption\""));
    }

    #[test]
    fn test_format_summary() {
        // Caption and author.
        let meta = PageMetadata {
            og_title: Some(MEDIA_TITLE.to_string()),
            og_description: Some("1,234 likes, 56 comments".to_string()),
        };
        assert_eq!(
            format_summary(&meta, "reel", 150).as_deref(),
            Some("“\x0fSome caption & more\x0310” is an Instagram reel by\x0f user.name")
        );

        // Long captions are truncated.
        let long_caption = "a".repeat(200);
        let meta = PageMetadata {
            og_title: Some(format!("user.name on Instagram: \"{long_caption}\"")),
            og_description: None,
        };
        let expected = format!(
            "“\x0f{}\x0310” is an Instagram post by\x0f user.name",
            "a".repeat(150) + "…"
        );
        assert_eq!(format_summary(&meta, "post", 150).as_deref(), Some(expected.as_str()));

        // An empty caption falls back to the description.
        let meta = PageMetadata {
            og_title: Some("user.name on Instagram: \"\"".to_string()),
            og_description: Some("description text".to_string()),
        };
        assert_eq!(
            format_summary(&meta, "post", 150).as_deref(),
            Some("“\x0fdescription text\x0310” is an Instagram post by\x0f user.name")
        );

        // A caption without an author.
        let meta = PageMetadata {
            og_title: Some("Just a title".to_string()),
            og_description: None,
        };
        assert_eq!(
            format_summary(&meta, "post", 150).as_deref(),
            Some("“\x0fJust a title\x0310” is an Instagram post")
        );

        // Login walls carry no summary.
        let meta = PageMetadata {
            og_title: Some(GENERIC_TITLE.to_string()),
            og_description: Some("1,234 likes".to_string()),
        };
        assert_eq!(format_summary(&meta, "reel", 150), None);
    }

    /// Drives the plugin's pipeline against a real Instagram link.
    ///
    /// Needs network access, so it is `#[ignore]`d. Instagram serves a login wall to requests it
    /// scores as bot traffic — e.g. from datacenter IPs — so a missing summary is not a failure:
    /// when the page metadata is fetched, the summary is asserted on; otherwise the test logs the
    /// outcome and passes.
    #[tokio::test]
    #[ignore = "needs network access"]
    async fn live_media_link() {
        let url = Url::parse("https://www.instagram.com/p/DdUGmgAifcq").unwrap();

        let link = parse_instagram_url(&url)
            .expect("the link parses as an Instagram link");
        assert_eq!(
            link,
            InstagramLink::Media {
                kind: MediaKind::Post,
                id: "DdUGmgAifcq".to_string(),
            }
        );

        let client = wreq::Client::builder()
            .emulation(Emulation::Firefox142)
            .redirect(Policy::limited(4))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("the http client builds");

        let InstagramLink::Media { kind, id } = link else {
            panic!("expected a media link, got {link:?}");
        };

        let canonical = media_url(kind, &id);
        println!("parsed: {kind:?} {id}, canonical url: {canonical}");

        match meta::fetch(&client, &canonical).await {
            Ok(metadata) => {
                println!("metadata: {metadata:?}");

                match format_summary(&metadata, kind.label(), 150) {
                    Some(summary) => {
                        assert!(summary.contains("is an Instagram post"), "summary: {summary}");
                        println!("summary: {summary}");
                    }
                    None => println!("no summary — the page is a login wall"),
                }
            }
            Err(error) => println!("metadata fetch failed (bot wall): {error}"),
        }
    }
}
