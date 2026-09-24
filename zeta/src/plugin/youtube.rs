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
//! (e.g. `ofn`) to identify video links.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use num_format::{Locale, ToFormattedString};
use serde::{Deserialize, Serialize};
use tokio::time::MissedTickBehavior;
use tracing::{Instrument, debug, warn};
use indefinite::indefinite_article_only;
use url::Url;

use crate::{
    cache::TtlCache,
    config::HttpConfig,
    duration::{format_duration, parse_iso8601_duration},
    http,
    plugin::prelude::*,
    url::ExtractUrlsExt,
};

mod urls;

pub use self::urls::{UrlKind, parse_youtube_url};

/// YouTube Data API v3 base endpoint URL.
const BASE_URL: &str = "https://www.googleapis.com/youtube/v3";

/// The time-to-live of the cached video categories.
const CATEGORIES_TTL: Duration = Duration::from_hours(24);

/// How often the categories task rechecks the cache for staleness.
///
/// A tick only fetches when the cached entry is missing or past its TTL, so the actual API
/// calls happen at most once per [`CATEGORIES_TTL`]; the shorter check interval makes a failed
/// fetch retry within an hour instead of after a full TTL.
const CATEGORIES_CHECK_INTERVAL: Duration = Duration::from_hours(1);

/// The `.yt` command.
const YOUTUBE: CommandSpec = CommandSpec::new(".yt", "Search YouTube and link the top video");

/// The safe search filter applied to YouTube search requests.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SafeSearch {
    /// Do not filter search results.
    #[default]
    None,
    /// Filter out content that is explicitly flagged as mature.
    Moderate,
    /// Filter out most potentially mature content.
    Strict,
}

impl SafeSearch {
    /// Returns the value of the `safeSearch` API parameter.
    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Moderate => "moderate",
            Self::Strict => "strict",
        }
    }
}

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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_key: None,
            region_code: "US".to_string(),
            safe_search: SafeSearch::default(),
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
    /// started in [`Plugin::loaded`] once per [`CATEGORIES_TTL`].
    video_categories: Arc<TtlCache<HashMap<String, Category>>>,
}

/// YouTube API and plugin-specific error types.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The API responded with a non-success status or an invalid body.
    #[error(transparent)]
    Api(#[from] http::ApiError),
    /// The search or video lookup matched nothing.
    #[error("no results")]
    NoResults,
}

/// Basic details about the video, such as its title and category.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    /// The video title, possibly containing HTML entities.
    pub title: String,
    /// The title of the channel that uploaded the video.
    pub channel_title: String,
    /// The video category id, resolvable through the video categories map.
    pub category_id: String,
    /// Whether the video is a `"live"`, `"upcoming"`, or regular broadcast.
    pub live_broadcast_content: String,
}

/// Statistics about a video.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Statistics {
    /// The number of times the video has been viewed, as a decimal string.
    pub view_count: String,
}

/// Details about the content of a video, such as its duration.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentDetails {
    /// The video duration as an ISO 8601 duration string (e.g. `PT4M13S`).
    pub duration: String,
}

/// Details about a live stream.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStreamingDetails {
    /// The number of concurrent viewers, present while the stream is live.
    pub concurrent_viewers: Option<String>,
}

/// A YouTube video.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Video {
    /// The video's basic details, present in the `snippet` API part.
    pub snippet: Option<Snippet>,
    /// The video's statistics, present in the `statistics` API part.
    pub statistics: Option<Statistics>,
    /// The video's content details, present in the `contentDetails` API part.
    pub content_details: Option<ContentDetails>,
    /// The video's live streaming details, present for live and upcoming streams.
    pub live_streaming_details: Option<LiveStreamingDetails>,
}

/// Search Result.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Search {
    /// The id of the matched resource, typed by its kind.
    pub id: SearchId,
    /// The matched resource's basic details.
    pub snippet: SearchSnippet,
}

/// The id of a search result, typed by the kind of resource the search matched.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchId {
    /// The video id, present for video results.
    pub video_id: Option<String>,
}

/// The snippet of a search result.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSnippet {
    /// The matched resource's title, possibly containing HTML entities.
    pub title: String,
}

/// Details about a video category.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategorySnippet {
    /// The category title, e.g. "Music".
    pub title: String,
}

/// A video category result.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    /// The category id.
    pub id: String,
    /// The category's details.
    pub snippet: CategorySnippet,
}

/// Generic response type for list results.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ApiListResponse<R> {
    /// The list items.
    pub items: Vec<R>,
}

/// Response with a list of YouTube videos.
pub type VideosResponse = ApiListResponse<Video>;

/// Response with a list of YouTube video categories.
pub type CategoriesResponse = ApiListResponse<Category>;

/// Response with a list of YouTube search results.
pub type SearchListResponse = ApiListResponse<Search>;

#[async_trait]
impl Plugin<Context> for YouTube {
    type Settings = Settings;

    fn new(
        ctx: &Context,
        settings: &Settings,
        subscriptions: &mut Subscriptions,
    ) -> Result<YouTube, ZetaError> {
        subscriptions
            .command(YOUTUBE)
            .urls(UrlScope::Hosts(urls::URL_HOSTS));

        let api_key = resolve_secret(settings.api_key.as_deref(), "YOUTUBE_API_KEY")?;

        Ok(YouTube::with_config(settings, api_key, &ctx.config.http))
    }

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        self.start_categories_refresh();

        Ok(())
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let args = command.args();

        // An invocation whose arguments are a YouTube URL resolves the video below instead of
        // searching for the URL as a query.
        if args.urls().next().is_some() {
            return Ok(());
        }

        match self.search(args).await {
            Ok(results) => {
                if let Some((result, id)) = results.iter().find_map(|result| {
                    let id = result.id.video_id.as_deref()?;

                    Some((result, id))
                }) {
                    let title = htmlize::unescape(&result.snippet.title);

                    client.send_privmsg(
                        channel,
                        reply(
                            "YouTube",
                            format!("{title} - https://www.youtube.com/watch?v={id}"),
                        ),
                    )?;
                } else {
                    client.send_privmsg(channel, notice("No results"))?;
                }
            }
            Err(err) => {
                client.send_privmsg(channel, notice(err))?;
            }
        }

        Ok(())
    }

    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &Client,
        url: &UrlEvent,
    ) -> Result<(), ZetaError> {
        self.process_url(url.url(), url.channel(), client).await?;

        Ok(())
    }
}

impl YouTube {
    /// Creates a new plugin instance for the resolved `api_key` and the settings it configures.
    #[must_use]
    pub fn with_config(settings: &Settings, api_key: String, config: &HttpConfig) -> Self {
        let client = http::build_client(config);

        Self {
            api_key,
            region_code: settings.region_code.clone(),
            safe_search: settings.safe_search,
            client,
            video_categories: Arc::new(TtlCache::new(CATEGORIES_TTL)),
        }
    }

    /// Spawns the task that periodically refreshes the cached video categories.
    ///
    /// The task ticks once per [`CATEGORIES_CHECK_INTERVAL`], skipping missed ticks: a tick is
    /// only an actual API call when the cached entry is missing or past its TTL, so the
    /// categories are fetched at most once per [`CATEGORIES_TTL`] and a failed fetch is retried
    /// on the next tick. Refresh failures are logged and leave the stale map in place.
    fn start_categories_refresh(&self) {
        let (client, api_key, region_code, cache) = (
            self.client.clone(),
            self.api_key.clone(),
            self.region_code.clone(),
            Arc::clone(&self.video_categories),
        );

        tokio::spawn(
            async move {
                debug!("starting video category refresh task");

                // The first tick completes immediately, populating the cache at startup.
                let mut interval = tokio::time::interval(CATEGORIES_CHECK_INTERVAL);
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

                loop {
                    interval.tick().await;

                    let refreshed = cache
                        .refresh(|| async {
                            fetch_video_categories(&client, &api_key, &region_code).await
                        })
                        .await;

                    if let Err(error) = refreshed {
                        warn!(%error, "could not refresh video categories");
                    }
                }
            }
            .instrument(tracing::info_span!("video_category_refresh")),
        );
    }

    /// Processes a URL found in a message.
    async fn process_url(&self, url: &Url, channel: &str, client: &Client) -> Result<(), ZetaError> {
        if let Some(UrlKind::Video(video_id) | UrlKind::Short(video_id)) = parse_youtube_url(url) {
            match self.get_video(&video_id).await {
                Ok(video) => {
                    let snippet = video.snippet.as_ref();
                    let category_id = snippet.map_or(String::new(), |s| s.category_id.clone());

                    // The map is refreshed by the task started on load; the read is
                    // stale-tolerant, so an unavailable API keeps serving the last known
                    // categories.
                    let category = self.video_categories.read(|cache| {
                        cache
                            .and_then(|categories| categories.get(&category_id))
                            .map_or_else(
                                || "unknown category".to_string(),
                                |category| category.snippet.title.clone(),
                            )
                    });

                    let view_count = video
                        .statistics
                        .as_ref()
                        .and_then(|s| str::parse::<u64>(&s.view_count).ok())
                        .unwrap_or(0);

                    let message = format_video_message(&video, &category, view_count);
                    client.send_privmsg(channel, message)?;
                }
                Err(e) => {
                    client.send_privmsg(channel, notice(e))?;
                }
            }
        }

        Ok(())
    }

    /// Searches for videos using the given query.
    async fn search(&self, query: &str) -> Result<Vec<Search>, Error> {
        debug!(%query, "searching for videos");

        let params = [
            ("q", query),
            ("key", &self.api_key),
            ("part", "snippet"),
            ("type", "video"),
            ("safeSearch", self.safe_search.as_str()),
        ];

        let request = self.client.get(format!("{BASE_URL}/search")).query(&params);

        let result: SearchListResponse = http::get_json(request).await?;

        let items = result.items;

        debug!(?items, "returning items");

        Ok(items)
    }

    /// Fetches metadata for a YouTube video using its video ID.
    ///
    /// Returns `Err(Error::NoResults)` if no video is found with the given ID.
    async fn get_video(&self, video_id: &str) -> Result<Video, Error> {
        debug!(%video_id, "fetching video metadata");

        let params = [
            ("id", video_id),
            ("key", &self.api_key),
            (
                "part",
                "snippet,statistics,contentDetails,liveStreamingDetails",
            ),
        ];
        let request = self.client.get(format!("{BASE_URL}/videos")).query(&params);
        let list: VideosResponse = http::get_json(request).await?;

        debug!("fetched metadata for video");

        list.items.into_iter().next().ok_or(Error::NoResults)
    }
}

/// Fetches the video categories map from the API, keyed by category id.
///
/// # Errors
///
/// Returns [`Error::Api`] if the request failed or the response could not be parsed, and
/// [`Error::NoResults`] if the API returned no categories.
async fn fetch_video_categories(
    client: &reqwest::Client,
    api_key: &str,
    region_code: &str,
) -> Result<HashMap<String, Category>, Error> {
    debug!("fetching video categories");

    let params = [
        ("key", api_key),
        ("part", "snippet"),
        ("regionCode", region_code),
    ];
    let request = client
        .get(format!("{BASE_URL}/videoCategories"))
        .query(&params);
    let list: CategoriesResponse = http::get_json(request).await?;

    debug!("fetched video category list");

    let map: HashMap<String, Category> =
        list.items.into_iter().map(|c| (c.id.clone(), c)).collect();

    if map.is_empty() {
        Err(Error::NoResults)
    } else {
        Ok(map)
    }
}

/// Formats the message describing `video`, using the resolved `category` and `view_count`.
///
/// Live and upcoming streams are described as live streams along with their category and
/// current number of concurrent viewers (falling back to the total view count when
/// unavailable), while regular videos include only their duration.
fn format_video_message(video: &Video, category: &str, view_count: u64) -> String {
    let snippet = video.snippet.as_ref();
    let title = snippet.map_or("‽", |s| s.title.as_str());
    let channel_name = snippet.map_or("unknown channel", |s| s.channel_title.as_str());
    let view_count_formatted = view_count.to_formatted_string(&Locale::en);

    let is_live_stream =
        snippet.is_some_and(|s| matches!(s.live_broadcast_content.as_str(), "live" | "upcoming"));

    if is_live_stream {
        let article = indefinite_article_only(category);

        if let Some(viewers) = video
            .live_streaming_details
            .as_ref()
            .and_then(|details| details.concurrent_viewers.as_deref())
            .and_then(|viewers| viewers.parse::<u64>().ok())
        {
            return notice(format!(
                "“\x0f{title}\x0310” is {article}\x0f {category}\x0310 live stream by\x0f \
                 {channel_name}\x0310 with\x0f {}\x0310 viewers",
                viewers.to_formatted_string(&Locale::en),
            ));
        }

        return notice(format!(
            "“\x0f{title}\x0310” is {article}\x0f {category}\x0310 live stream by\x0f \
             {channel_name}\x0310 with\x0f {view_count_formatted}\x0310 views",
        ));
    }

    let duration = video
        .content_details
        .as_ref()
        .and_then(|details| parse_iso8601_duration(&details.duration))
        .map_or_else(|| "unknown duration".to_string(), format_duration);
    let article = indefinite_article_only(&duration);

    notice(format!(
        "“\x0f{title}\x0310” is {article}\x0f {duration}\x0310 video by\x0f \
         {channel_name}\x0310 with\x0f {view_count_formatted}\x0310 views",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.api_key.is_none());
            assert_eq!(settings.region_code, "US");
            assert_eq!(settings.safe_search, SafeSearch::None);
        }
        deserialize: {
            "api_key": "secret",
            "region_code": "DK",
            "safe_search": "strict",
        } assert: {
            assert_eq!(settings.api_key.as_deref(), Some("secret"));
            assert_eq!(settings.region_code, "DK");
            assert_eq!(settings.safe_search, SafeSearch::Strict);
        }
    }

    /// Returns a video with the given live status, duration, and concurrent viewer count.
    fn test_video(
        live_broadcast_content: &str,
        duration: Option<&str>,
        concurrent_viewers: Option<&str>,
    ) -> Video {
        Video {
            snippet: Some(Snippet {
                title: "Test Video".to_string(),
                channel_title: "Test Channel".to_string(),
                category_id: "10".to_string(),
                live_broadcast_content: live_broadcast_content.to_string(),
            }),
            statistics: None,
            content_details: duration.map(|duration| ContentDetails {
                duration: duration.to_string(),
            }),
            live_streaming_details: concurrent_viewers.map(|viewers| LiveStreamingDetails {
                concurrent_viewers: Some(viewers.to_string()),
            }),
        }
    }

    #[test]
    fn formats_video_message() {
        let video = test_video("none", Some("PT1H2M20S"), None);

        assert_eq!(
            format_video_message(&video, "Music", 123_456),
            "\x0310> “\x0fTest Video\x0310” is a\x0f 1h 2m 20s\x0310 video by\x0f Test Channel\x0310 with\x0f 123,456\x0310 views",
        );
    }

    #[test]
    fn formats_video_message_with_unknown_duration() {
        let video = test_video("none", None, None);

        assert_eq!(
            format_video_message(&video, "Music", 1),
            "\x0310> “\x0fTest Video\x0310” is an\x0f unknown duration\x0310 video by\x0f Test Channel\x0310 with\x0f 1\x0310 views",
        );
    }

    #[test]
    fn formats_live_stream_message() {
        let video = test_video("live", None, Some("1234"));

        assert_eq!(
            format_video_message(&video, "Music", 42),
            "\x0310> “\x0fTest Video\x0310” is a\x0f Music\x0310 live stream by\x0f Test Channel\x0310 with\x0f 1,234\x0310 viewers",
        );
    }

    #[test]
    fn formats_live_stream_message_with_vowel_category() {
        let video = test_video("live", None, Some("1234"));

        assert_eq!(
            format_video_message(&video, "Education", 42),
            "\x0310> “\x0fTest Video\x0310” is an\x0f Education\x0310 live stream by\x0f Test Channel\x0310 with\x0f 1,234\x0310 viewers",
        );
    }

    #[test]
    fn formats_upcoming_stream_message_with_total_views() {
        let video = test_video("upcoming", None, None);

        assert_eq!(
            format_video_message(&video, "Music", 42),
            "\x0310> “\x0fTest Video\x0310” is a\x0f Music\x0310 live stream by\x0f Test Channel\x0310 with\x0f 42\x0310 views",
        );
    }
}
