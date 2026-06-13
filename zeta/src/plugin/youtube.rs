#![allow(clippy::doc_markdown)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use num_format::{Locale, ToFormattedString};
use tokio::sync::RwLock;
use url::Url;

use crate::{
    http,
    plugin::{self, prelude::*},
};

mod api;
mod types;
mod urls;

use types::Category;
use urls::UrlKind;

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
    /// The `.yt` IRC command
    command: Prefix,
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
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("no results")]
    NoResults,
    #[error("deserialization error: {0}")]
    Deserialize(#[source] serde_path_to_error::Error<serde_json::Error>),
}

#[async_trait]
impl Plugin<Context> for YouTube {
    fn new(_ctx: &Context) -> Result<YouTube, ZetaError> {
        let api_key = require_env("YOUTUBE_API_KEY")?;

        Ok(YouTube::with_config(api_key))
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "youtube".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command {
            if let Some(urls) = plugin::extract_urls(user_message) {
                self.process_urls(urls, channel, client).await?;
            } else if let Some(args) = self.command.parse(user_message) {
                match self.search(args).await {
                    Ok(results) => {
                        if let Some(result) = results.first() {
                            let id = result.id.video_id.as_ref().unwrap();
                            let title = &result.snippet.title;

                            client.send_privmsg(channel, format!("\x0310>\x03\x02 YouTube:\x02\x0310 {title} - https://www.youtube.com/watch?v={id}"))?;
                        } else {
                            client.send_privmsg(channel, "\x0310> No results")?;
                        }
                    }
                    Err(err) => {
                        client.send_privmsg(channel, format!("\x0310> Error: {err}"))?;
                    }
                }
            }
        }

        Ok(())
    }
}

impl YouTube {
    pub fn with_config(api_key: String) -> Self {
        let client = http::build_client();
        let command = Prefix::new(".yt");

        Self {
            api_key,
            client,
            command,
            video_categories: RwLock::new(Arc::new(HashMap::new())),
            video_categories_updated_at: RwLock::new(None),
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
                urls::parse_youtube_url(url)
            {
                match self.get_video(&video_id).await {
                    Ok(video) => {
                        let snippet = video.snippet.as_ref();
                        let statistics = video.statistics.as_ref();
                        let title = snippet.map_or_else(|| "‽".to_string(), |s| s.title.clone());
                        let category_id = snippet.map_or(String::new(), |s| s.category_id.clone());
                        let categories = self.cached_video_categories().await.unwrap();
                        // TODO: use indefinite form: https://crates.io/crates/indefinite
                        let category = categories.get(&category_id).map_or_else(
                            || "unknown category".to_string(),
                            |s| s.snippet.title.clone(),
                        );
                        let channel_name = snippet.map_or_else(
                            || "unknown channel".to_string(),
                            |s| s.channel_title.clone(),
                        );
                        let view_count = statistics
                            .and_then(|s| str::parse::<u64>(&s.view_count).ok())
                            .unwrap_or(0);
                        let view_count_formatted = view_count.to_formatted_string(&Locale::en);

                        client
                        .send_privmsg(channel, format!("\x0310> “\x0f{title}\x0310” is a\x0f {category}\x0310 video by\x0f {channel_name}\x0310 with\x0f {view_count_formatted}\x0310 views"))?;
                    }
                    Err(e) => {
                        client.send_privmsg(channel, format!("Error: {e}"))?;
                    }
                }
            }
        }

        Ok(())
    }
}
