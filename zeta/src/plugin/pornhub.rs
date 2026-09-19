//! Expands PornHub links with video details.
//!
//! `view_video.php?viewkey=<id>` links on `www.pornhub.com` are looked up through the legacy
//! webmasters API and replied to with the video title and view count. A missing video maps to
//! a not-found error; every other lookup failure is silently ignored, leaving the link
//! unexpanded.
//!
//! The plugin has no settings.

use num_format::{Locale, ToFormattedString};
use serde::Deserialize;
use tracing::debug;
use url::Url;

use crate::{http, plugin::prelude::*, url::query_param};

/// The hostname for PornHub URLs.
const PORNHUB_HOST: &str = "www.pornhub.com";
/// The error code returned by the API when a video is not found.
const ERROR_CODE_NOT_FOUND: &str = "1002";

/// Plugin for handling PornHub video URLs and fetching video metadata.
pub struct PornHub {
    /// The inner HTTP client.
    client: reqwest::Client,
}

/// Errors that can occur when interacting with the PornHub API.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Sending the HTTP request failed.
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    /// The API response could not be deserialized.
    #[error("could not deserialize response: {0}")]
    Deserialize(#[source] serde_path_to_error::Error<serde_json::Error>),
    /// The requested video does not exist.
    #[error("resource not found")]
    NotFound,
    /// The API returned an error other than a missing video.
    #[error("invalid response")]
    InvalidResponse,
}

/// Response from the PornHub API, either containing video data or an error.
#[derive(Deserialize, Debug)]
#[serde(untagged)]
#[allow(dead_code)]
enum ApiResponse {
    /// API error response with error details.
    Error {
        code: String,
        message: Option<String>,
        example: Option<String>,
    },
    /// Successful response containing video information.
    Video {
        /// Contains all the details about the video.
        video: Box<Video>,
    },
}

/// Represents the main video object with all its metadata.
#[derive(Deserialize, Debug)]
#[allow(clippy::struct_field_names, dead_code)]
pub struct Video {
    /// The duration of the video in "MM:SS" or "HH:MM:SS" format.
    pub duration: String,
    /// The total number of views.
    pub views: u64,
    /// The unique identifier for the video.
    pub video_id: String,
    /// The rating percentage (e.g., 92.3872).
    pub rating: f64,
    /// The total number of ratings submitted.
    pub ratings: u64,
    /// The title of the video.
    pub title: String,
    /// The URL to the video page.
    pub url: String,
    /// The URL of the default thumbnail image.
    pub default_thumb: String,
    /// The URL of the primary thumbnail image.
    pub thumb: String,
    /// The publication date and time in "YYYY-MM-DD HH:MM:SS" format.
    pub publish_date: String,
    /// A list of available thumbnail images.
    pub thumbs: Vec<Thumb>,
    /// A list of tags associated with the video.
    pub tags: Vec<Tag>,
    /// A list of pornstars featured in the video. Can be empty.
    pub pornstars: Vec<Pornstar>,
    /// A list of categories the video belongs to.
    pub categories: Vec<Category>,
    /// The segment or market (e.g., "straight").
    pub segment: String,
}

/// Represents a single thumbnail image with its properties.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct Thumb {
    /// The size of the thumbnail in "`WIDTHxHEIGHT`" format.
    pub size: String,
    /// The width of the thumbnail in pixels, as a string.
    pub width: String,
    /// The height of the thumbnail in pixels, as a string.
    pub height: String,
    /// The source URL of the thumbnail image.
    pub src: String,
}

/// Represents a tag associated with the video.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct Tag {
    /// The name of the tag.
    pub tag_name: String,
}

/// Represents a pornstar featured in the video.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct Pornstar {
    /// The name of the pornstar.
    pub pornstar_name: String,
}

/// Represents a category the video is classified under.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct Category {
    /// The name of the category.
    pub category: String,
}

#[async_trait]
impl Plugin<Context> for PornHub {
    type Settings = NoSettings;

    /// Creates a new instance of the PornHub plugin.
    fn new(ctx: &Context, _settings: &NoSettings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.urls(UrlScope::Hosts(&[PORNHUB_HOST]));

        let client = http::build_client(&ctx.config.http);

        Ok(PornHub { client })
    }

    /// Processes incoming PornHub URLs.
    async fn handle_url(&self, _ctx: &Context, client: &Client, url: &UrlEvent) -> Result<(), ZetaError> {
        let _ = self.process_url(url.url(), url.channel(), client).await;

        Ok(())
    }
}

impl PornHub {
    // Processes a single URL if it's a valid PornHub video URL.
    async fn process_url(&self, url: &Url, channel: &str, client: &Client) -> Result<(), Error> {
        if is_pornhub_video_url(url)
            && let Some(video_id) = query_param(url, "viewkey")
        {
            debug!(%video_id, "processing video");
            let video = self.video_by_id(&video_id).await?;
            debug!(?video, "fetched video");

            let _ = client.send_privmsg(channel, Self::format_video_message(&video));
        }

        Ok(())
    }

    /// Fetches video information by its ID from the PornHub API.
    ///
    async fn video_by_id(&self, video_id: &str) -> Result<Video, Error> {
        let url = Url::parse_with_params(
            "https://www.pornhub.com/webmasters/video_by_id",
            [("id", video_id)],
        )
        .unwrap();

        let response = self.client.get(url).send().await.map_err(Error::Request)?;
        debug!("request went ok, parsing response");
        let text = response.text().await.map_err(Error::Request)?;
        let json: ApiResponse = http::json::from_str(&text).map_err(Error::Deserialize)?;

        match json {
            ApiResponse::Error { code, .. } => {
                if code == ERROR_CODE_NOT_FOUND {
                    Err(Error::NotFound)
                } else {
                    Err(Error::InvalidResponse)
                }
            }
            ApiResponse::Video { video } => Ok(*video),
        }
    }

    /// Formats a message about the video.
    fn format_video_message(video: &Video) -> String {
        let title = &video.title;
        let views = video.views.to_formatted_string(&Locale::en);

        notice(format!(
            "“\x0f{title}\x0310” is a PornHub video with\x0f {views}\x0310 views"
        ))
    }
}

/// Checks if the URL is a valid PornHub video URL.
///
/// The dispatcher only delivers URLs whose host matches the plugin's registered host, so only
/// the path is checked here.
#[must_use]
pub fn is_pornhub_video_url(url: &Url) -> bool {
    url.path() == "/view_video.php"
}
