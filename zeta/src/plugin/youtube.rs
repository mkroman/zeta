use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use num_format::{Locale, ToFormattedString};
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::debug;
use url::Url;

use super::{Author, Name, Plugin, Version};
use crate::{plugin, Error as ZetaError};

/// YouTube Data API v3 base endpoint URL.
pub const BASE_URL: &str = "https://www.googleapis.com/youtube/v3";

/// IRC bot plugin for YouTube URL detection and metadata retrieval.
///
/// This plugin monitors IRC messages for YouTube URLs and automatically responds
/// with video metadata including title, category, channel name, and view count.
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
    /// HTTP client for making API requests with connection pooling
    client: reqwest::Client,
    /// Thread-safe cache of video categories mapped by category ID
    video_categories: RwLock<Arc<HashMap<String, Category>>>,
    /// Timestamp tracking when video categories were last fetched for cache invalidation
    video_categories_updated_at: RwLock<Option<Instant>>,
}

/// YouTube API and plugin-specific error types.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("server returned invalid response")]
    InvalidResponse,
    #[error("request error")]
    Request(#[from] reqwest::Error),
    #[error("no results")]
    NoResults,
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
}

/// Statistics about a video.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct Statistics {
    pub view_count: String,
}

/// A YouTube video.
#[derive(Clone, Debug, Deserialize)]
#[allow(unused)]
pub struct Video {
    pub kind: String,
    pub etag: String,
    pub id: String,
    pub snippet: Option<Snippet>,
    pub statistics: Option<Statistics>,
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

#[async_trait]
impl Plugin for YouTube {
    fn new() -> YouTube {
        let api_key =
            std::env::var("YOUTUBE_API_KEY").expect("missing YOUTUBE_API_KEY environment variable");

        YouTube::with_config(api_key)
    }

    fn name() -> Name {
        Name("youtube")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command {
            if let Some(urls) = extract_urls(user_message) {
                self.process_urls(urls, channel, client).await?;
            }
        }

        Ok(())
    }
}

impl YouTube {
    pub fn with_config(api_key: String) -> Self {
        let client = plugin::build_http_client();

        Self {
            api_key,
            client,
            video_categories: RwLock::new(Arc::new(HashMap::new())),
            video_categories_updated_at: RwLock::new(None),
        }
    }

    /// Parses the given `url` and returns a [`UrlKind`] depending on the type of YouTube URL.
    fn parse_youtube_url(url: &Url) -> Option<UrlKind> {
        match url.host_str()? {
            "youtu.be" => parse_youtu_be_url(url),
            "youtube.com" | "www.youtube.com" => parse_youtube_com_url(url),
            _ => None,
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
                YouTube::parse_youtube_url(url)
            {
                match self.get_video(&video_id).await {
                    Ok(video) => {
                        let snippet = video.snippet.as_ref();
                        let statistics = video.statistics.as_ref();
                        let title = snippet.map_or("‽".to_string(), |s| s.title.clone());
                        let category_id = snippet.map_or(String::new(), |s| s.category_id.clone());
                        let categories = self.cached_video_categories().await.unwrap();
                        let category = categories
                            .get(&category_id)
                            .map_or("unknown category".to_string(), |s| s.snippet.title.clone());
                        let channel_name = snippet
                            .map_or("unknown channel".to_string(), |s| s.channel_title.clone());
                        let view_count = statistics
                            .and_then(|s| str::parse::<u64>(&s.view_count).ok())
                            .unwrap_or(0);
                        let view_count_formatted = view_count.to_formatted_string(&Locale::en);

                        client
                        .send_privmsg(channel, format!("\x0310> “\x0f{title}\x0310” is a\x0f {category}\x0310 video by\x0f {channel_name}\x0310 with\x0f {view_count_formatted}\x0310 views"))
                        .map_err(ZetaError::IrcClientError)?;
                    }
                    Err(e) => {
                        client
                            .send_privmsg(channel, format!("Error: {e}"))
                            .map_err(ZetaError::IrcClientError)?;
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
            ("regionCode", "US"),
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
        if let Some(instant) = *self.video_categories_updated_at.read().await {
            debug!("using cached video categories");

            if instant.elapsed() < Duration::from_secs(30 * 60) {
                let vc = self.video_categories.read().await;

                return Ok(vc.clone());
            }
        }

        debug!("refreshing cached video categories");
        let new_categories = self.video_categories().await?;
        let categories_arc = Arc::new(new_categories);

        {
            let mut categories_guard = self.video_categories.write().await;
            *categories_guard = categories_arc.clone();
        }
        {
            let mut updated_at_guard = self.video_categories_updated_at.write().await;
            *updated_at_guard = Some(Instant::now());
        }

        let vc = self.video_categories.read().await;
        Ok(vc.clone())
    }

    /// Fetches metadata for a YouTube video using its video ID.
    ///
    /// Returns `Err(Error::NoResults)` if no video is found with the given ID.
    async fn get_video(&self, video_id: &str) -> Result<Video, Error> {
        debug!(%video_id, "fetching video metadata");

        let params = [
            ("id", video_id),
            ("key", &self.api_key),
            ("part", "snippet,statistics,liveStreamingDetails"),
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

fn extract_urls(s: &str) -> Option<Vec<Url>> {
    let urls: Vec<Url> = s
        .split(' ')
        .filter(|word| word.to_ascii_lowercase().starts_with("http"))
        .filter_map(|word| Url::parse(word).ok())
        .collect();

    if urls.is_empty() {
        None
    } else {
        Some(urls)
    }
}

/// Extracts a query parameter value from a URL
fn extract_query_param(url: &Url, param: &str) -> Option<String> {
    url.query_pairs()
        .find(|(key, _)| key == param)
        .map(|(_, value)| value.to_string())
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

    #[test]
    fn it_should_extract_https_urls() {
        assert_eq!(
            extract_urls("nice nok https://github.com/dani-garcia/vaultwarden/pull/3899"),
            Some(vec![Url::parse(
                "https://github.com/dani-garcia/vaultwarden/pull/3899"
            )
            .unwrap()])
        );
    }

    #[test]
    fn test_parse_youtube_com_video_urls() {
        let test_cases = [
            (
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
            ),
            (
                "https://youtube.com/watch?v=dQw4w9WgXcQ",
                Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
            ),
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(YouTube::parse_youtube_url(&url), expected);
        }
    }

    #[test]
    fn test_parse_youtube_com_shorts_urls() {
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

            assert_eq!(YouTube::parse_youtube_url(&url), expected);
        }
    }

    #[test]
    fn test_parse_youtu_be_video_urls() {
        let test_cases = [(
            "https://youtu.be/dQw4w9WgXcQ",
            Some(UrlKind::Video("dQw4w9WgXcQ".to_string())),
        )];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(YouTube::parse_youtube_url(&url), expected);
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

            assert_eq!(YouTube::parse_youtube_url(&url), expected);
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
            assert_eq!(YouTube::parse_youtube_url(&url), None);
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

            assert_eq!(YouTube::parse_youtube_url(&url), expected);
        }
    }
}
