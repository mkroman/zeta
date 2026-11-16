//! Expands YouTube links with video details and searches YouTube for videos.
//!
//! Video links — `youtube.com/watch?v=<id>`, `youtube.com/shorts/<id>`, and `youtu.be/<id>` —
//! are resolved through the YouTube Data API v3 and replied to with the title, duration,
//! category, channel, and view count; live and upcoming streams are described as such along
//! with their concurrent viewer count (falling back to the total view count). Playlist,
//! channel, and handle URLs are parsed but deliberately left to other plugins.
//!
//! The `.yt <query>` command searches YouTube and posts the top video's title with a `watch?v=`
//! link. An invocation whose arguments are themselves a YouTube URL resolves it as a link
//! below instead of searching for it as a query, so the two event kinds do not double up.
//!
//! The video category map is cached in memory with a 24-hour TTL, keyed by the `region_code`
//! setting (default `US`); a task started when the plugin loads refreshes it periodically, so
//! URL handling never triggers or waits on a refresh. The `safe_search` setting (default
//! `none`) is passed to search requests. The API key is set in `[plugins.youtube]`, falling
//! back to the `YOUTUBE_API_KEY` environment variable; a missing key fails plugin
//! initialization and the plugin is skipped at startup.
//!
//! The URL parser behind link detection is [`parse_youtube_url`], reused by other plugins
//! (e.g. `ofn`) to identify video links. It lives in the shared `plugins_shared::urls::youtube` module
//! so those plugins can share it without enabling this plugin.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use plugins_shared::urls::youtube::URL_HOSTS;
use zeta_cache::TtlCache;
use zeta_http as http;
use zeta_http::reqwest;
use zeta_plugin::{Client, CommandEvent, Context, UrlEvent, ZetaError, plugin, resolve_secret};

mod api;
mod categories;
mod commands;
mod format;
mod model;
mod urls;

#[cfg(test)]
mod tests;

pub use format::SafeSearch;
pub use model::{
    ApiListResponse, CategoriesResponse, Category, CategorySnippet, ContentDetails,
    LiveStreamingDetails, Search, SearchId, SearchListResponse, SearchSnippet, Snippet, Statistics,
    Video, VideosResponse,
};
pub use plugins_shared::urls::youtube::{UrlKind, parse_youtube_url};

use self::categories::CATEGORIES_TTL;

/// Settings for the youtube plugin, from its `[plugins.youtube]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The YouTube Data API v3 key.
    ///
    /// Falls back to the `YOUTUBE_API_KEY` environment variable when unset.
    pub api_key: Option<String>,
    /// The region code used when fetching video categories.
    pub region_code: String,
    /// The safe search filter applied to search requests.
    pub safe_search: SafeSearch,
    /// Enable the `.yt` command.
    pub enable_command: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_key: None,
            region_code: "US".to_string(),
            safe_search: SafeSearch::default(),
            enable_command: true,
        }
    }
}

/// IRC bot plugin for YouTube URL detection and metadata retrieval.
///
/// This plugin monitors IRC messages for YouTube URLs and automatically responds
/// with video metadata including title, duration, channel name, and view count.
/// It maintains a cache of YouTube video categories to reduce API calls and
/// uses async/await for non-blocking operation.
///
/// # Features
/// - Automatic URL detection in IRC messages
/// - Video metadata extraction via YouTube Data API v3
/// - Thread-safe category caching with expiration, refreshed periodically in the background
/// - Support for multiple YouTube URL formats
/// - Formatted output with IRC color codes
pub struct YouTube {
    /// YouTube Data API v3 authentication key
    api_key: String,
    /// The region code used when fetching video categories
    region_code: String,
    /// The safe search filter applied to search requests
    safe_search: SafeSearch,
    /// HTTP client for making API requests with connection pooling
    client: reqwest::Client,
    /// Thread-safe cache of video categories mapped by category ID, refreshed by the task
    /// started in [`Plugin::loaded`](zeta_plugin::Plugin::loaded) once per [`CATEGORIES_TTL`].
    ///
    /// The map is shared behind an `Arc` so a reply borrows the titles instead of cloning one
    /// per video.
    video_categories: Arc<TtlCache<Arc<HashMap<String, Category>>>>,
}

// Manual implementation: the API key and the opaque HTTP client are not printed, and the cache
// is summarized by whether it holds the category map.
impl fmt::Debug for YouTube {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("YouTube")
            .field("region_code", &self.region_code)
            .field("safe_search", &self.safe_search)
            .field(
                "categories_cached",
                &self
                    .video_categories
                    .with_value(|categories| categories.is_some()),
            )
            .finish_non_exhaustive()
    }
}

/// YouTube API and plugin-specific error types.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The API request failed, or the response could not be read or parsed.
    #[error("the youtube request failed")]
    Api(#[from] http::ApiError),
    /// The search or video lookup matched nothing.
    #[error("no results")]
    NoResults,
}

#[plugin(name = "youtube", settings = Settings, urls(hosts = URL_HOSTS))]
impl YouTube {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let api_key = resolve_secret(settings.api_key.as_deref(), "YOUTUBE_API_KEY")?;
        let client = ctx.http().clone();

        Ok(Self {
            api_key,
            region_code: settings.region_code.clone(),
            safe_search: settings.safe_search,
            client,
            video_categories: Arc::new(TtlCache::new(CATEGORIES_TTL)),
        })
    }

    #[handler(loaded)]
    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        self.start_categories_refresh();

        Ok(())
    }

    /// Replies to `.yt` with the top video matching the query.
    ///
    /// An invocation whose arguments are themselves a YouTube URL resolves it as a link below
    /// instead of searching for it as a query, so the two event kinds do not double up.
    #[command(
        ".yt",
        "Search YouTube and link the top video",
        event,
        usage_hint = "<query>",
        register_if = settings.enable_command
    )]
    async fn search_video(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        commands::search_video(self, client, command).await
    }

    /// Replies to a posted YouTube video link with the video's details.
    #[handler(url)]
    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &Client,
        url: &UrlEvent,
    ) -> Result<(), ZetaError> {
        commands::handle_url(self, client, url).await
    }
}
