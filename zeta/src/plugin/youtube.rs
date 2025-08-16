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

use crate::{plugin, Error as ZetaError};

use super::{Author, Name, Plugin, Version};

pub const API_BASE_URL: &str = "https://www.googleapis.com/youtube/v3";

pub struct YouTube {
    api_key: String,
    client: reqwest::Client,
    video_categories: RwLock<Arc<HashMap<String, VideoCategory>>>,
    video_categories_updated_at: RwLock<Option<Instant>>,
}

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
    /// Direct link to a channel using channel ID (e.g., `youtube.com/channel/CHANNEL_ID`)
    Channel(String),
    /// Link to a channel using the @ handle (e.g., `youtube.com/@ChannelName`)
    ChannelHandle(String),
    /// Direct link to a playlist (e.g., `youtube.com/playlist?list=PLAYLIST_ID`)
    Playlist(String),
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct VideoSnippet {
    pub title: String,
    pub description: String,
    pub channel_title: String,
    pub category_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct VideoStatistics {
    pub view_count: String,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(unused)]
pub struct Video {
    pub kind: String,
    pub etag: String,
    pub id: String,
    pub snippet: Option<VideoSnippet>,
    pub statistics: Option<VideoStatistics>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(unused)]
pub struct VideoCategorySnippet {
    pub channel_id: String,
    pub title: String,
    pub assignable: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(unused)]
#[serde(rename_all = "camelCase")]
pub struct VideoCategory {
    pub kind: String,
    pub etag: String,
    pub id: String,
    pub snippet: VideoCategorySnippet,
}

#[derive(Deserialize, Debug)]
#[allow(unused)]
pub struct ListResponse<R> {
    pub kind: String,
    pub etag: String,
    pub items: Vec<R>,
}

pub type VideoListResponse = ListResponse<Video>;
pub type VideoCategoryListResponse = ListResponse<VideoCategory>;

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
            if let Some(UrlKind::Video(video_id)) = YouTube::parse_youtube_url(url) {
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
    async fn video_categories(&self) -> Result<HashMap<String, VideoCategory>, Error> {
        debug!("fetching video categories");

        let params = [
            ("key", self.api_key.as_str()),
            ("part", "snippet"),
            ("regionCode", "US"),
        ];
        let request = self
            .client
            .get(format!("{API_BASE_URL}/videoCategories"))
            .query(&params);
        let response = request
            .send()
            .await
            .map_err(|_| Error::InvalidResponse)?
            .error_for_status()?;
        let list: VideoCategoryListResponse = response.json().await?;

        debug!("fetched video category list");

        let map: HashMap<String, VideoCategory> =
            list.items.into_iter().map(|c| (c.id.clone(), c)).collect();

        if map.is_empty() {
            Err(Error::NoResults)
        } else {
            Ok(map)
        }
    }

    async fn cached_video_categories(&self) -> Result<Arc<HashMap<String, VideoCategory>>, Error> {
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
        let params = [
            ("id", video_id),
            ("key", &self.api_key),
            ("part", "snippet,statistics,liveStreamingDetails"),
        ];
        let request = self
            .client
            .get(format!("{API_BASE_URL}/videos"))
            .query(&params);
        debug!("fetching metadata for video");
        let response = request
            .send()
            .await
            .map_err(|_| Error::InvalidResponse)?
            .error_for_status()?;
        let list: VideoListResponse = response.json().await?;
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
