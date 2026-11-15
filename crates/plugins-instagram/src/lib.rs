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
//! The module is split by concern: `media` holds URL routing and summary/mirror orchestration,
//! `details` the metadata fetching and request throttling, and `format` the summary text;
//! `graphql`, `urls`, and `meta` hold the API client, URL classification, and OpenGraph scraping.
//!
//! [InstaFix]: https://github.com/Wikidepia/InstaFix

mod details;
mod format;
mod graphql;
mod media;
mod meta;
mod urls;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::warn;

use zeta_cache::{TtlCache, TtlMap};
use zeta_http as http;
use zeta_http::WreqError;
use zeta_http::emulated::ResponseError;
use zeta_http::wreq;
use zeta_http::wreq::redirect::Policy;
use zeta_mirror::MirrorHandle;
use zeta_plugin::{Chain, Client, Context, UrlEvent, ZetaError, plugin, plugin_err};
use zeta_utils::settings;

use self::meta::MediaDetails;

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
    #[serde(alias = "title_length")]
    pub caption_length: usize,
    /// The prefix and public URL base of mirrored media.
    #[serde(flatten)]
    pub mirror: plugins_shared::mirror::MirrorSettings,
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
            caption_length: 150,
            mirror: plugins_shared::mirror::MirrorSettings::default(),
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
    ///
    /// Shared rather than copied: a gated fetch only needs to borrow the tokens, and an `Arc`
    /// clone is a refcount bump instead of two `String` allocations.
    session: TtlCache<Arc<graphql::SessionTokens>>,
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

impl From<ResponseError> for Error {
    fn from(error: ResponseError) -> Self {
        match error {
            ResponseError::Request(error) => Self::Request(error),
            // A share link that resolves to an error page does not identify any media.
            ResponseError::Status(_) => Self::InvalidRedirect,
        }
    }
}

#[plugin(name = "instagram", settings = Settings, urls(hosts = urls::URL_HOSTS))]
impl Instagram {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client = http::emulated::builder(None)
            .map_err(plugin_err)?
            .redirect(Policy::limited(4))
            .timeout(ctx.config().http.timeout)
            .build()
            .map_err(|error| plugin_err(WreqError::from(error)))?;

        let mirror = settings.mirror.resolve(ctx, "instagram");

        let session_cookie = settings::resolve_optional_setting(
            settings.session_cookie.as_deref(),
            "INSTAGRAM_SESSION_COOKIE",
        );
        let metadata_proxy = settings::resolve_optional_setting(
            settings.metadata_proxy.as_deref(),
            "INSTAGRAM_METADATA_PROXY",
        );

        Ok(Self {
            client,
            mirror,
            settings: settings.clone(),
            session_cookie,
            metadata_proxy,
            last_request: Mutex::new(None),
            session: TtlCache::new(SESSION_TTL),
            details: TtlMap::new_with_negative_ttl(
                DETAILS_TTL,
                MISSING_DETAILS_TTL,
                DETAILS_CACHE_CAPACITY,
            ),
        })
    }

    #[handler(url)]
    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &Client,
        url: &UrlEvent,
    ) -> Result<(), ZetaError> {
        if let Err(err) = self.process_url(url.url(), url.channel(), client).await {
            warn!(exception.message = %Chain(&err), "could not process url");
        }

        Ok(())
    }
}
