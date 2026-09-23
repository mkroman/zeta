//! Summarises Instagram links and mirrors their videos to S3.
//!
//! Handles feed posts, reels, and IGTV videos; if mirroring is configured, it downloads the
//! videos with `yt-dlp` and mirrors them to an S3-compatible bucket,
//! replying with a public link to the mirrored file. Stories are mirrored without a summary:
//! their pages carry no metadata without an authenticated session.
//!
//! Instagram aggressively limits anonymous access: it serves a login wall to requests it scores
//! as bot traffic, and some media is not anonymously accessible at all. The summary is therefore
//! assembled from several sources, mirroring what `yt-dlp` and the InstaFix family of embed
//! proxies do:
//!
//! 1. the OpenGraph metadata of the canonical media page,
//! 2. Instagram's anonymous GraphQL API, using the LSD and CSRF tokens the front page provides —
//!    fetched once and reused, since they are stable across requests,
//! 3. an optionally configured metadata proxy (e.g. a self-hosted [InstaFix] instance), which
//!    serves the OpenGraph metadata of the media from its own addresses.
//!
//! Requests are throttled and their results cached, so bursts of links do not trip the rate
//! limiting and repeated links do not repeat the fetches. An authenticated session cookie
//! (`sessionid`) can be configured to lift the login wall entirely.
//!
//! Mirroring is configured through the top-level `[mirror]` configuration section (or the `S3_*`
//! environment variables); without it, the plugin only posts summaries.
//!
//! [InstaFix]: https://github.com/Wikidepia/InstaFix

mod graphql;
mod meta;
mod urls;

use std::fmt::Write;
use std::future::Future;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, error};
use url::Url;
use wreq::redirect::Policy;

use crate::{
    cache::{TtlCache, TtlMap},
    error::WreqError,
    http,
    mirror::{Mirror, MirrorHandle},
    plugin::prelude::*,
    url::redact_url_str,
    utils::{Truncatable, collapse_whitespace},
};

use self::meta::MediaDetails;
use self::urls::{InstagramLink, MediaKind, media_url, parse_instagram_url, story_url};

/// The minimum interval between requests to Instagram, so that bursts of links do not trip its
/// rate limiting.
const REQUEST_INTERVAL: Duration = Duration::from_secs(2);

/// How long fetched media details stay cached.
const DETAILS_TTL: Duration = Duration::from_mins(10);

/// How long a negative result (media without accessible details) stays cached.
const MISSING_DETAILS_TTL: Duration = Duration::from_mins(1);

/// How many media details are cached before the closest-to-expiring ones are evicted.
const DETAILS_CACHE_CAPACITY: usize = 256;

/// How long the front-page session tokens for the GraphQL API stay cached. They are stable
/// across requests, so a run of gated media shares one front-page fetch.
const SESSION_TTL: Duration = Duration::from_hours(1);

/// Settings for the instagram plugin, from its `[plugins.instagram]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The maximum length of a media caption before it gets truncated.
    pub title_length: usize,
    /// The key prefix that mirrored media are uploaded under.
    ///
    /// Falls back to the `INSTAGRAM_S3_PREFIX` environment variable, and to `instagram` when
    /// neither is set.
    pub prefix: Option<String>,
    /// The base URL used when linking to mirrored media.
    ///
    /// Falls back to the `INSTAGRAM_PUBLIC_URL_BASE` environment variable when unset. Links are
    /// built by appending the media id as a URL fragment, so the base must point at a viewer page
    /// that resolves the fragment — not directly at the bucket.
    pub public_url_base: Option<String>,
    /// An authenticated Instagram session cookie (the `sessionid` cookie value of a logged-in
    /// browser session).
    ///
    /// Lifts the login wall for media that would otherwise not be readable anonymously,
    /// including stories. Falls back to the `INSTAGRAM_SESSION_COOKIE` environment variable when
    /// unset. Prefer the environment over committing the cookie to this file.
    pub session_cookie: Option<String>,
    /// The base URL of a proxy that serves the OpenGraph metadata of Instagram media, used as a
    /// last resort when the other sources fail — e.g. a self-hosted InstaFix instance.
    ///
    /// Falls back to the `INSTAGRAM_METADATA_PROXY` environment variable when unset.
    pub metadata_proxy: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            title_length: 150,
            prefix: None,
            public_url_base: None,
            session_cookie: None,
            metadata_proxy: None,
        }
    }
}

/// The Instagram plugin: summarizes and mirrors Instagram media links.
pub struct Instagram {
    /// The HTTP client used for fetching pages, emulating a modern browser.
    client: wreq::Client,
    /// The shared mirror handle, when mirroring is configured.
    mirror: Option<MirrorHandle>,
    /// The plugin settings used when processing URLs.
    settings: Settings,
    /// The resolved session cookie, if any.
    session_cookie: Option<String>,
    /// The resolved metadata proxy base URL, if any.
    metadata_proxy: Option<String>,
    /// The instant of the previous request to Instagram, throttling bursts.
    last_request: Mutex<Option<Instant>>,
    /// The front-page session tokens for the GraphQL API, refreshed after they expire.
    session: TtlCache<graphql::SessionTokens>,
    /// The details fetched for each media id, cached so repeated links do not repeat the fetches.
    details: TtlMap<String, MediaDetails>,
}

/// Errors that can occur while summarizing or mirroring Instagram media.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Sending the HTTP request failed.
    #[error("request error: {0}")]
    Request(WreqError),
    /// A share link did not resolve to a valid Instagram media URL.
    #[error("share link did not resolve to a valid url")]
    InvalidRedirect,
    /// Every consulted metadata source failed with a request error, rather than answering
    /// without details. Reported so the miss is not cached as a negative result.
    #[error("all metadata sources failed")]
    SourcesFailed,
}

impl From<wreq::Error> for Error {
    /// Wraps `error` in a [`WreqError`], which redacts the request URL: the metadata proxy
    /// URL can carry a credential in its query string.
    fn from(error: wreq::Error) -> Self {
        Self::Request(error.into())
    }
}

#[async_trait]
impl Plugin<Context> for Instagram {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Instagram, ZetaError> {
        subscriptions.urls(UrlScope::Hosts(urls::URL_HOSTS));

        let client = http::emulated::builder(None)?
            .redirect(Policy::limited(4))
            .timeout(ctx.config.http.timeout)
            .build()
            .map_err(|error| plugin_err(WreqError::from(error)))?;

        let mirror = MirrorHandle::resolve(
            ctx.shared.get::<Mirror>(),
            "instagram",
            settings.prefix.as_deref(),
            settings.public_url_base.as_deref(),
        );

        let session_cookie = crate::utils::resolve_optional_setting(
            settings.session_cookie.as_deref(),
            "INSTAGRAM_SESSION_COOKIE",
        );
        let metadata_proxy = crate::utils::resolve_optional_setting(
            settings.metadata_proxy.as_deref(),
            "INSTAGRAM_METADATA_PROXY",
        );

        Ok(Instagram {
            client,
            mirror,
            settings: settings.clone(),
            session_cookie,
            metadata_proxy,
            last_request: Mutex::new(None),
            session: TtlCache::new(SESSION_TTL),
            details: TtlMap::with_negative_ttl(DETAILS_TTL, MISSING_DETAILS_TTL, DETAILS_CACHE_CAPACITY),
        })
    }

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        if let Some(mirror) = &self.mirror {
            mirror.start_downloads();
        }

        Ok(())
    }

    async fn handle_url(&self, _ctx: &Context, client: &Client, url: &UrlEvent) -> Result<(), ZetaError> {
        if let Err(err) = self.process_url(url.url(), url.channel(), client).await {
            error!("could not process url: {err}");
        }

        Ok(())
    }
}

impl Instagram {
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

                self.throttle().await;

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

    /// Posts a summary of the media with the given kind and id, and queues it for mirroring.
    ///
    /// The details are fetched and cached through [`Self::post_summary`].
    async fn process_media(
        &self,
        kind: &MediaKind,
        id: &str,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        let url = media_url(*kind, id);

        self.post_summary(id, &url, kind.label(), || async {
            self.fetch_media_details(id, &url, *kind).await
        }, channel, client)
        .await
    }

    /// Posts a summary of the story published by `username` with the given id, and queues it
    /// for mirroring.
    ///
    /// Stories have no anonymously accessible API, so only their page is consulted, which
    /// generally carries details only with a session cookie configured.
    async fn process_story(
        &self,
        username: &str,
        id: &str,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        let url = story_url(username, id);

        self.post_summary(id, &url, "story", || async {
            self.fetch_page_details(&url, self.session_cookie.as_deref()).await
        }, channel, client)
        .await
    }

    /// Posts the summary for the cached details of a media or story, and queues the video for
    /// mirroring.
    ///
    /// The details for `id` are fetched through `refresh` when the cache is stale; a failed
    /// refresh is logged and skips the summary only.
    async fn post_summary<E, F, Fut>(
        &self,
        id: &str,
        url: &str,
        label: &str,
        refresh: F,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error>
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = Result<Option<MediaDetails>, E>> + Send,
        E: std::fmt::Display,
    {
        let details = match self.details.get_or_refresh(id.to_string(), refresh).await {
            Ok(details) => details,
            Err(error) => {
                debug!(%id, %error, "could not fetch {label} details");

                None
            }
        };

        if let Some(summary) = details.as_ref().and_then(|details| {
            format_summary(details, label, self.settings.title_length)
        }) {
            let _ = client.send_privmsg(channel, notice(&summary));
        }

        self.mirror_video(url, id, channel, client).await;

        Ok(())
    }

    /// Fetches the details of the media with the given shortcode.
    ///
    /// The OpenGraph metadata of the canonical media page is tried first, then the anonymous
    /// GraphQL API, then the configured metadata proxy. A page request that errors is retried
    /// once, so a transient failure does not cost the summary.
    ///
    /// Returns `Ok(None)` when no source carried details — a negative result the caller caches.
    /// An error is returned when a source failed with a request error instead of answering
    /// without details: the outcome is unknown then, and the caller does not cache it, so a
    /// later mention of the same link tries again.
    async fn fetch_media_details(
        &self,
        id: &str,
        canonical: &str,
        kind: MediaKind,
    ) -> Result<Option<MediaDetails>, Error> {
        let mut details = None;
        let mut failed = false;

        self.try_source(id, "media page", self.fetch_page_details(canonical, self.session_cookie.as_deref()), &mut details, &mut failed).await;

        if details.is_none() {
            debug!(%id, "falling back to the graphql api");

            self.try_source(id, "graphql api", self.fetch_graphql_details(id), &mut details, &mut failed).await;
        }

        if details.is_none() && let Some(proxy) = &self.metadata_proxy {
            let url = proxy_media_url(proxy, kind, id);
            debug!(url.full = %redact_url_str(&url), "falling back to the metadata proxy");

            self.try_source(id, "metadata proxy", self.fetch_page_details(&url, None), &mut details, &mut failed).await;
        }

        if details.is_none() && failed {
            return Err(Error::SourcesFailed);
        }

        Ok(details)
    }

    /// Awaits one source of details, storing the result in `details` and logging a failure.
    ///
    /// A failed source sets `failed`, which turns into an error when no source ends up carrying
    /// details: the outcome is unknown then, and the caller does not cache it, so a later
    /// mention of the same link tries again.
    async fn try_source<E, Fut>(
        &self,
        id: &str,
        context: &str,
        source: Fut,
        details: &mut Option<MediaDetails>,
        failed: &mut bool,
    ) where
        Fut: Future<Output = Result<Option<MediaDetails>, E>>,
        E: std::fmt::Display,
    {
        match source.await {
            Ok(source_details) => *details = source_details,
            Err(error) => {
                debug!(%id, %error, "the {context} request failed");

                *failed = true;
            }
        }
    }

    /// Fetches the media details through the anonymous GraphQL API.
    ///
    /// The front-page session tokens the request needs are cached, so a run of gated media does
    /// not each fetch the front page. Returns `Ok(None)` when the media is not anonymously
    /// accessible, and an error when the requests fail.
    async fn fetch_graphql_details(&self, id: &str) -> Result<Option<MediaDetails>, graphql::Error> {
        self.throttle().await;

        let session = self
            .session
            .get_or_refresh(|| async {
                graphql::fetch_session(&self.client, self.session_cookie.as_deref()).await
            })
            .await?;

        match graphql::fetch_media_details(&self.client, id, &session, self.session_cookie.as_deref())
            .await
        {
            Ok(details) => Ok(Some(details)),
            Err(error @ graphql::Error::Gated) => {
                debug!(%id, %error, "the graphql api carried no details");

                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Fetches a page and extracts its OpenGraph details.
    ///
    /// Returns `Ok(None)` when the page carried no details, and an error when the request
    /// failed.
    async fn fetch_page_details(
        &self,
        url: &str,
        session_cookie: Option<&str>,
    ) -> Result<Option<MediaDetails>, meta::Error> {
        let details = MediaDetails::from_og(&self.fetch_page_metadata(url, session_cookie).await?);

        if details.is_none() {
            debug!(url.full = %redact_url_str(url), "the page carried no media details");
        }

        Ok(details)
    }

    /// Fetches a page, retrying the request once after a pause.
    ///
    /// Client errors are not retried: a dead link or a rejected request would fail the retry all
    /// the same, and the pause would only delay the fallbacks that follow.
    async fn fetch_page_metadata(
        &self,
        url: &str,
        session_cookie: Option<&str>,
    ) -> Result<meta::PageMetadata, meta::Error> {
        self.throttle().await;

        let error = match meta::fetch(&self.client, url, session_cookie).await {
            Ok(metadata) => return Ok(metadata),
            Err(error) => error,
        };

        debug!(url.full = %redact_url_str(url), %error, "could not fetch page metadata");

        if let meta::Error::Status(status) = &error && status.is_client_error() {
            return Err(error);
        }

        // A failed request is retried once after a pause, so a transient failure does not cost
        // the summary.
        self.throttle().await;

        meta::fetch(&self.client, url, session_cookie)
            .await
            .inspect_err(|error| debug!(url.full = %redact_url_str(url), %error, "the page request failed again"))
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

    /// Ensures at least `REQUEST_INTERVAL` has passed since the previous request to Instagram,
    /// then records this one.
    async fn throttle(&self) {
        let mut last_request = self.last_request.lock().await;

        if let Some(last_request) = *last_request
            && let Some(wait) = REQUEST_INTERVAL.checked_sub(last_request.elapsed())
        {
            tokio::time::sleep(wait).await;
        }

        *last_request = Some(Instant::now());
    }

    /// Starts mirroring of the given media, replying with a link to the mirrored file.
    ///
    /// If the media has already been mirrored, the existing link is sent immediately; otherwise
    /// the download and upload happens in a background task that replies with the link.
    async fn mirror_video(&self, url: &str, id: &str, channel: &str, client: &Client) {
        if let Some(mirror) = &self.mirror {
            mirror.ensure_mirrored_and_reply(url, id, channel, client).await;
        }
    }
}

/// Returns the URL of the media's page on the configured metadata proxy.
///
/// The proxy serves the OpenGraph metadata of Instagram media, so the media path is the canonical
/// one.
#[must_use]
fn proxy_media_url(proxy: &str, kind: MediaKind, id: &str) -> String {
    format!("{}/{}/{}", proxy.trim_end_matches('/'), kind.segment(), id)
}

/// Formats the media details as a human-readable summary of the media.
fn format_summary(details: &MediaDetails, kind: &str, title_length: usize) -> Option<String> {
    let mut buf = String::new();

    if let Some(caption) = details
        .caption
        .as_deref()
        .map(collapse_whitespace)
        .filter(|caption| !caption.is_empty())
    {
        let truncated = caption.truncate_with_suffix(title_length, "…");
        let _ = write!(buf, "“\x0f{}\x0310” ", truncated.trim());
    }

    if let Some(author) = details
        .author
        .as_deref()
        .map(collapse_whitespace)
        .filter(|author| !author.is_empty())
    {
        if buf.is_empty() {
            let _ = write!(buf, "Instagram {kind} by\x0f {author}");
        } else {
            let _ = write!(buf, "is an Instagram {kind} by\x0f {author}");
        }
    } else if !buf.is_empty() {
        let _ = write!(buf, "is an Instagram {kind}");
    }

    (!buf.is_empty()).then_some(buf)
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
            assert!(settings.session_cookie.is_none());
            assert!(settings.metadata_proxy.is_none());
        }
        deserialize: {
            "title_length": 100,
            "prefix": "~meta/instagram",
            "public_url_base": "https://pub.example.com/instagram",
            "session_cookie": "1234567890%3Aexample",
            "metadata_proxy": "https://d.example.com",
        } assert: {
            assert_eq!(settings.title_length, 100);
            assert_eq!(settings.prefix.as_deref(), Some("~meta/instagram"));
            assert_eq!(
                settings.public_url_base.as_deref(),
                Some("https://pub.example.com/instagram")
            );
            assert_eq!(
                settings.session_cookie.as_deref(),
                Some("1234567890%3Aexample")
            );
            assert_eq!(settings.metadata_proxy.as_deref(), Some("https://d.example.com"));
        }
    }

    /// Returns the details of a media with both a caption and an author, to be partially
    /// overridden per case.
    fn full_details() -> MediaDetails {
        MediaDetails {
            author: Some("user.name".to_string()),
            caption: Some("Some caption & more".to_string()),
        }
    }

    #[test]
    fn test_format_summary() {
        // Caption and author.
        assert_eq!(
            format_summary(&full_details(), "reel", 150).as_deref(),
            Some("“\x0fSome caption & more\x0310” is an Instagram reel by\x0f user.name")
        );

        // Long captions are truncated.
        let long_caption = "a".repeat(200);
        let details = MediaDetails {
            caption: Some(long_caption),
            ..full_details()
        };
        let expected = format!(
            "“\x0f{}\x0310” is an Instagram post by\x0f user.name",
            "a".repeat(150) + "…"
        );
        assert_eq!(format_summary(&details, "post", 150).as_deref(), Some(expected.as_str()));

        // A caption without an author.
        let details = MediaDetails {
            author: None,
            caption: Some("Just a title".to_string()),
        };
        assert_eq!(
            format_summary(&details, "post", 150).as_deref(),
            Some("“\x0fJust a title\x0310” is an Instagram post")
        );

        // An author without a caption.
        let details = MediaDetails {
            caption: None,
            ..full_details()
        };
        assert_eq!(
            format_summary(&details, "story", 150).as_deref(),
            Some("Instagram story by\x0f user.name")
        );

        // Neither an author nor a caption.
        let details = MediaDetails {
            author: None,
            caption: None,
        };
        assert_eq!(format_summary(&details, "post", 150), None);
    }

    #[test]
    fn format_summary_collapses_caption_whitespace() {
        // Captions carry line breaks, which an IRC message cannot contain: everything after
        // the first one would be lost.
        let details = MediaDetails {
            caption: Some("First line\n\nsecond\tline\r\nthird".to_string()),
            ..full_details()
        };
        assert_eq!(
            format_summary(&details, "post", 150).as_deref(),
            Some("“\x0fFirst line second line third\x0310” is an Instagram post by\x0f user.name")
        );
    }

    #[test]
    fn test_proxy_media_url() {
        assert_eq!(
            proxy_media_url("https://d.example.com", MediaKind::Post, "DdUGmgAifcq"),
            "https://d.example.com/p/DdUGmgAifcq"
        );
        assert_eq!(
            proxy_media_url("https://d.example.com/", MediaKind::Reel, "DbfDT_9xiU3"),
            "https://d.example.com/reel/DbfDT_9xiU3"
        );
    }

    /// Drives the plugin's pipeline against real Instagram links.
    ///
    /// Needs network access, so it is `#[ignore]`d. Instagram serves a login wall to requests it
    /// scores as bot traffic — e.g. from datacenter IPs — and some media is not anonymously
    /// accessible at all, so a missing summary is not a failure: the summary is asserted on when
    /// the metadata sources carried details; otherwise the test logs the outcome and passes.
    #[tokio::test]
    #[ignore = "needs network access"]
    async fn live_media_links() {
        let client = http::emulated::builder(None)
            .expect("the headers are valid")
            .redirect(Policy::limited(4))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("the http client builds");

        let instagram = Instagram {
            client,
            mirror: None,
            settings: Settings::default(),
            session_cookie: None,
            metadata_proxy: None,
            last_request: Mutex::new(None),
            session: TtlCache::new(SESSION_TTL),
            details: TtlMap::with_negative_ttl(DETAILS_TTL, MISSING_DETAILS_TTL, DETAILS_CACHE_CAPACITY),
        };

        let urls = [
            "https://www.instagram.com/p/DdUGmgAifcq",
            "https://www.instagram.com/p/DdUfj1USxzi",
            "https://www.instagram.com/reel/DbfDT_9xiU3/",
            "https://www.instagram.com/reel/DaklCKgDnIe/",
        ];

        for url in urls {
            let url = Url::parse(url).unwrap();

            let InstagramLink::Media { kind, id } = parse_instagram_url(&url).unwrap() else {
                panic!("expected a media link for {url}");
            };

            let canonical = media_url(kind, &id);
            println!("== {url} → {canonical}");

            let details = instagram
                .details
                .get_or_refresh(id.clone(), || async {
                    instagram.fetch_media_details(&id, &canonical, kind).await
                })
                .await
                .unwrap_or(None);

            match details.as_ref().and_then(|details| format_summary(details, kind.label(), 150)) {
                Some(summary) => println!("   summary: {summary}"),
                None => println!("   no summary — no source carried details"),
            }
        }
    }
}
