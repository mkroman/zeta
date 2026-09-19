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
//! The video category map is cached in memory with a 30-minute TTL, keyed by the `region_code`
//! setting (default `US`); the `safe_search` setting (default `none`) is passed to search
//! requests. The API key is set in `[plugins.youtube]`, falling back to the `YOUTUBE_API_KEY`
//! environment variable; a missing key fails plugin initialization and the plugin is skipped
//! at startup.
//!
//! The URL parser behind link detection is [`parse_youtube_url`], reused by other plugins
//! (e.g. `ofn`) to identify video links.

#![allow(clippy::doc_markdown)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use num_format::{Locale, ToFormattedString};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::debug;
use url::Url;

use crate::{
    cache::TtlCache,
    config::HttpConfig,
    duration::{format_duration, parse_iso8601_duration},
    http,
    plugin::prelude::*,
    url::ExtractUrlsExt,
};

/// The hostname of shortened YouTube URLs.
const YOUTU_BE_HOST: &str = "youtu.be";

/// The YouTube.com hostname.
const YOUTUBE_COM_HOST: &str = "youtube.com";

/// The www-prefixed YouTube.com hostname.
const YOUTUBE_COM_WWW_HOST: &str = "www.youtube.com";

/// The YouTube hosts whose links this plugin handles.
const URL_HOSTS: &[&str] = &[YOUTU_BE_HOST, YOUTUBE_COM_HOST, YOUTUBE_COM_WWW_HOST];

/// YouTube Data API v3 base endpoint URL.
const BASE_URL: &str = "https://www.googleapis.com/youtube/v3";

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
pub struct Settings {
    /// The YouTube Data API v3 key.
    ///
    /// Falls back to the `YOUTUBE_API_KEY` environment variable when unset.
    #[serde(default)]
    pub api_key: Option<String>,
    /// The region code used when fetching video categories.
    #[serde(default = "default_region_code")]
    pub region_code: String,
    /// The safe search filter applied to search requests.
    #[serde(default)]
    pub safe_search: SafeSearch,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_key: None,
            region_code: default_region_code(),
            safe_search: SafeSearch::default(),
        }
    }
}

/// Returns the default region code for video categories.
fn default_region_code() -> String {
    "US".to_string()
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
/// - Thread-safe category caching with expiration
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
    /// Thread-safe cache of video categories mapped by category ID, refreshed after the cache
    /// TTL expires.
    video_categories: TtlCache<Arc<HashMap<String, Category>>>,
}

/// YouTube API and plugin-specific error types.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("server returned invalid response")]
    InvalidResponse,
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("no results")]
    NoResults,
    #[error("deserialization error: {0}")]
    Deserialize(#[source] serde_path_to_error::Error<serde_json::Error>),
}

#[derive(Eq, PartialEq, Debug)]
#[non_exhaustive]
pub enum UrlKind {
    /// Direct link to a video (e.g., `youtube.com/watch?v=VIDEO_ID` or `youtu.be/VIDEO_ID`)
    Video(String),
    /// Link to a short video (e.g., `youtube.com/shorts/VIDEO_ID`)
    Short(String),
    /// Direct link to a channel using channel ID (e.g., `youtube.com/channel/CHANNEL_ID`)
    Channel(String),
    /// Link to a channel using the @ handle (e.g., `youtube.com/@ChannelName`)
    ChannelHandle(String),
    /// Direct link to a playlist (e.g., `youtube.com/playlist?list=PLAYLIST_ID`)
    Playlist(String),
}

/// Basic details about the video, such as its title, description, and category.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct Snippet {
    pub title: String,
    pub description: String,
    pub channel_title: String,
    pub category_id: String,
    pub live_broadcast_content: String,
}

/// Statistics about a video.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct Statistics {
    pub view_count: String,
}

/// Details about the content of a video, such as its duration.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct ContentDetails {
    pub duration: String,
}

/// Details about a live stream.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct LiveStreamingDetails {
    /// The number of concurrent viewers, present while the stream is live.
    pub concurrent_viewers: Option<String>,
}

/// A YouTube video.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct Video {
    pub kind: String,
    pub etag: String,
    pub id: String,
    pub snippet: Option<Snippet>,
    pub statistics: Option<Statistics>,
    pub content_details: Option<ContentDetails>,
    pub live_streaming_details: Option<LiveStreamingDetails>,
}

/// Search Result.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct Search {
    pub kind: String,
    pub etag: String,
    pub id: SearchId,
    pub snippet: SearchSnippet,
}

// TODO: rework this so it uses an enum
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct SearchId {
    pub kind: String,
    pub video_id: Option<String>,
    pub channel_id: Option<String>,
    pub playlist_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct SearchSnippet {
    pub title: String,
    pub description: String,
    pub channel_id: String,
    pub channel_title: String,
    pub thumbnails: HashMap<String, SearchSnippetThumbnail>,
    #[serde(with = "time::serde::rfc3339")]
    pub published_at: OffsetDateTime,
    pub live_broadcast_content: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct SearchSnippetThumbnail {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

/// Details about a video category.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct CategorySnippet {
    pub channel_id: String,
    pub title: String,
    pub assignable: bool,
}

/// A video category result.
#[derive(Clone, Debug, Deserialize)]
#[allow(unused)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub kind: String,
    pub etag: String,
    pub id: String,
    pub snippet: CategorySnippet,
}

/// Generic response type for list results.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct ApiListResponse<R> {
    pub kind: String,
    pub etag: String,
    pub items: Vec<R>,
}

/// Response with a list of YouTube videos.
pub type VideosResponse = ApiListResponse<Video>;

/// Response with a list of YouTube video categories.
pub type CategoriesResponse = ApiListResponse<Category>;

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
            .urls(UrlScope::Hosts(URL_HOSTS));

        let api_key = resolve_secret(settings.api_key.as_deref(), "YOUTUBE_API_KEY")?;

        Ok(YouTube::with_config(settings, api_key, &ctx.config.http))
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
                if let Some(result) = results.first() {
                    let id = result.id.video_id.as_ref().unwrap();
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
                client.send_privmsg(channel, notice(format!("Error: {err}")))?;
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
        self.process_urls(vec![url.url().clone()], url.channel(), client)
            .await?;

        Ok(())
    }
}

impl YouTube {
    pub fn with_config(settings: &Settings, api_key: String, config: &HttpConfig) -> Self {
        let client = http::build_client(config);

        Self {
            api_key,
            region_code: settings.region_code.clone(),
            safe_search: settings.safe_search,
            client,
            video_categories: TtlCache::new(Duration::from_mins(30)),
        }
    }

    /// Processes URLs found in a message
    async fn process_urls(
        &self,
        urls: Vec<Url>,
        channel: &str,
        client: &Client,
    ) -> Result<(), ZetaError> {
        for ref url in urls {
            if let Some(UrlKind::Video(video_id) | UrlKind::Short(video_id)) =
                parse_youtube_url(url)
            {
                match self.get_video(&video_id).await {
                    Ok(video) => {
                        let snippet = video.snippet.as_ref();
                        let category_id = snippet.map_or(String::new(), |s| s.category_id.clone());
                        let categories = self.cached_video_categories().await.unwrap();
                        // TODO: use indefinite form: https://crates.io/crates/indefinite
                        let category = categories.get(&category_id).map_or_else(
                            || "unknown category".to_string(),
                            |s| s.snippet.title.clone(),
                        );
                        let view_count = video
                            .statistics
                            .as_ref()
                            .and_then(|s| str::parse::<u64>(&s.view_count).ok())
                            .unwrap_or(0);

                        let message = format_video_message(&video, &category, view_count);
                        client.send_privmsg(channel, message)?;
                    }
                    Err(e) => {
                        client.send_privmsg(channel, format!("Error: {e}"))?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Fetches video categories.
    async fn video_categories(&self) -> Result<HashMap<String, Category>, Error> {
        debug!("fetching video categories");

        let params = [
            ("key", self.api_key.as_str()),
            ("part", "snippet"),
            ("regionCode", self.region_code.as_str()),
        ];
        let request = self
            .client
            .get(format!("{BASE_URL}/videoCategories"))
            .query(&params);
        let response = request
            .send()
            .await
            .map_err(|_| Error::InvalidResponse)?
            .error_for_status()?;
        let list: CategoriesResponse = response.json().await?;

        debug!("fetched video category list");

        let map: HashMap<String, Category> =
            list.items.into_iter().map(|c| (c.id.clone(), c)).collect();

        if map.is_empty() {
            Err(Error::NoResults)
        } else {
            Ok(map)
        }
    }

    async fn cached_video_categories(&self) -> Result<Arc<HashMap<String, Category>>, Error> {
        self.video_categories
            .get_or_refresh(|| async {
                debug!("refreshing cached video categories");
                let categories = self.video_categories().await?;

                Ok(Arc::new(categories))
            })
            .await
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

        debug!(?params, "searching for videos");

        let request = self.client.get(format!("{BASE_URL}/search")).query(&params);
        let response = request.send().await?.error_for_status()?;

        debug!("response is ok, parsing as json");
        let text = response.text().await.map_err(Error::Request)?;
        let result: SearchListResponse = http::json::from_str(&text).map_err(Error::Deserialize)?;

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
        let response = request
            .send()
            .await
            .map_err(|_| Error::InvalidResponse)?
            .error_for_status()?;
        let list: VideosResponse = response.json().await?;
        debug!("fetched metadata for video");

        if let Some(video) = list.items.first() {
            return Ok(video.clone());
        }

        Err(Error::NoResults)
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
        let concurrent_viewers = video
            .live_streaming_details
            .as_ref()
            .and_then(|details| details.concurrent_viewers.as_deref())
            .and_then(|viewers| viewers.parse::<u64>().ok());

        if let Some(viewers) = concurrent_viewers {
            return notice(format!(
                "“\x0f{title}\x0310” is a\x0f {category}\x0310 live stream by\x0f \
                 {channel_name}\x0310 with\x0f {}\x0310 viewers",
                viewers.to_formatted_string(&Locale::en),
            ));
        }

        return notice(format!(
            "“\x0f{title}\x0310” is a\x0f {category}\x0310 live stream by\x0f \
             {channel_name}\x0310 with\x0f {view_count_formatted}\x0310 views",
        ));
    }

    let duration = video
        .content_details
        .as_ref()
        .and_then(|details| parse_iso8601_duration(&details.duration))
        .map_or_else(|| "unknown duration".to_string(), format_duration);

    notice(format!(
        "“\x0f{title}\x0310” is a\x0f {duration}\x0310 video by\x0f \
         {channel_name}\x0310 with\x0f {view_count_formatted}\x0310 views",
    ))
}

/// Extracts a query parameter value from a URL
fn extract_query_param(url: &Url, param: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == param)
        .map(|(_, value)| value.to_string())
}

/// Parses the given `url` and returns a [`UrlKind`] depending on the type of YouTube URL.
pub fn parse_youtube_url(url: &Url) -> Option<UrlKind> {
    match url.host_str()? {
        YOUTU_BE_HOST => parse_youtu_be_url(url),
        YOUTUBE_COM_HOST | YOUTUBE_COM_WWW_HOST => parse_youtube_com_url(url),
        _ => None,
    }
}

/// Parses youtube.com URLs
fn parse_youtube_com_url(url: &Url) -> Option<UrlKind> {
    let segments: Vec<&str> = url.path_segments()?.collect();

    match segments.as_slice() {
        // `/watch?v=<video_id>`
        ["watch"] => extract_query_param(url, "v").map(UrlKind::Video),
        // `/playlist?list=<playlist_id>`
        ["playlist"] => extract_query_param(url, "list").map(UrlKind::Playlist),
        // `/channel/<channel_id>`
        ["channel", channel_id] if !channel_id.is_empty() => {
            Some(UrlKind::Channel((*channel_id).to_string()))
        }
        ["shorts", video_id] if !video_id.is_empty() => {
            Some(UrlKind::Short((*video_id).to_string()))
        }
        // `/*`
        [path] if path.starts_with('@') && path.len() > 1 => {
            Some(UrlKind::ChannelHandle(path[1..].to_string()))
        }
        _ => None,
    }
}

/// Parses youtu.be URLs
fn parse_youtu_be_url(url: &Url) -> Option<UrlKind> {
    let path = url.path();

    if path.len() > 1 {
        return Some(UrlKind::Video(path[1..].to_owned()));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_parse_video_urls() {
        let test_cases = [
            (
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
            ),
            (
                "https://youtube.com/watch?v=dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
            ),
            (
                "https://youtu.be/dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
            ),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_youtube_url(&url), expected, "for {url_str}");
        }
    }

    #[test]
    fn test_parse_shorts_urls() {
        let test_cases = [
            (
                "https://www.youtube.com/shorts/l4s8y-O_ols",
                Some(UrlKind::Short("l4s8y-O_ols".to_string())),
            ),
            (
                "https://youtube.com/shorts/l4s8y-O_ols",
                Some(UrlKind::Short("l4s8y-O_ols".to_string())),
            ),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_youtube_url(&url), expected, "for {url_str}");
        }
    }

    #[test]
    fn test_parse_playlist_urls() {
        let test_cases = [(
            "https://www.youtube.com/playlist?list=PLF37D334894B07EEA",
            Some(UrlKind::Playlist("PLF37D334894B07EEA".to_string())),
        )];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_youtube_url(&url), expected);
        }
    }

    #[test]
    fn test_invalid_urls() {
        let invalid_urls = [
            "https://example.com/watch?v=test",
            "https://youtube.com/channel/",
            "https://youtu.be/",
        ];

        for url_str in invalid_urls {
            let url = Url::parse(url_str).unwrap();
            assert_eq!(parse_youtube_url(&url), None);
        }
    }

    #[test]
    fn it_should_parse_channel_urls() {
        let test_cases = [
            (
                "https://www.youtube.com/channel/UChuZAo1RKL85gev3Eal9_zg",
                Some(UrlKind::Channel("UChuZAo1RKL85gev3Eal9_zg".to_string())),
            ),
            (
                "https://www.youtube.com/@BreakingTaps",
                Some(UrlKind::ChannelHandle("BreakingTaps".to_string())),
            ),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(parse_youtube_url(&url), expected);
        }
    }

    /// Returns a video with the given live status, duration, and concurrent viewer count.
    fn test_video(
        live_broadcast_content: &str,
        duration: Option<&str>,
        concurrent_viewers: Option<&str>,
    ) -> Video {
        Video {
            kind: "youtube#video".to_string(),
            etag: String::new(),
            id: "dQw4w9WgXcQ".to_string(),
            snippet: Some(Snippet {
                title: "Test Video".to_string(),
                description: String::new(),
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
            "\x0310> “\x0fTest Video\x0310” is a\x0f unknown duration\x0310 video by\x0f Test Channel\x0310 with\x0f 1\x0310 views",
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
    fn formats_upcoming_stream_message_with_total_views() {
        let video = test_video("upcoming", None, None);

        assert_eq!(
            format_video_message(&video, "Music", 42),
            "\x0310> “\x0fTest Video\x0310” is a\x0f Music\x0310 live stream by\x0f Test Channel\x0310 with\x0f 42\x0310 views",
        );
    }
}
