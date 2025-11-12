//! PornHub platform integration.
//!
//! This plugin provides functionality to display information about linked PornHub videos.

use num_format::{Locale, ToFormattedString};
use reqwest::Response;
use serde::{Deserialize, de::DeserializeOwned};
use tracing::{debug, error};
use url::Url;

use crate::{
    http,
    plugin::{self, prelude::*},
};

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
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("could not deserialize response: {0}")]
    Deserialize(#[source] serde_path_to_error::Error<serde_json::Error>),
    #[error("resource not found")]
    NotFound,
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
    /// The size of the thumbnail in "WIDTHxHEIGHT" format.
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
impl Plugin for PornHub {
    /// Creates a new instance of the PornHub plugin.
    fn new() -> Self {
        let client = http::build_client();

        PornHub { client }
    }

    fn name() -> Name {
        Name::from("pornhub")
    }

    fn author() -> Author {
        Author::from("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version::from("0.1")
    }

    // Handles incoming messages and processes any PornHub URLs found.
    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(urls) = plugin::extract_urls(user_message)
        {
            let _ = self.process_urls(urls, channel, client).await;
        }

        Ok(())
    }
}

impl PornHub {
    /// Processes multiple URLs, handling each one that matches the PornHub pattern.
    async fn process_urls(
        &self,
        urls: Vec<Url>,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        for url in &urls {
            debug!(%url, "processing url");
            self.process_url(url, channel, client).await?;
            debug!(%url, "finished processing url");
        }

        Ok(())
    }

    // Processes a single URL if it's a valid PornHub video URL.
    async fn process_url(&self, url: &Url, channel: &str, client: &Client) -> Result<(), Error> {
        if is_pornhub_video_url(url)
            && let Some(video_id) = extract_video_id(url)
        {
            debug!(%video_id, "processing video");
            let video = self.video_by_id(&video_id).await?;
            debug!(?video, "fetched video");

            let _ = client.send_privmsg(channel, Self::format_video_mesage(&video));
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
        let json: ApiResponse = deserialize_response(response).await?;

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
    fn format_video_mesage(video: &Video) -> String {
        let title = &video.title;
        let views = video.views.to_formatted_string(&Locale::en);

        format!("\x0310> “\x0f{title}\x0310” is a PornHub video with\x0f {views}\x0310 views")
    }
}

/// Checks if the URL is a valid PornHub video URL.
pub fn is_pornhub_video_url(url: &Url) -> bool {
    url.host_str() == Some(PORNHUB_HOST) && url.path() == "/view_video.php"
}

/// Extracts the video ID from a PornHub URL.
fn extract_video_id(url: &Url) -> Option<String> {
    url.query_pairs().find_map(|(key, value)| {
        if key == "viewkey" {
            Some(value.into_owned())
        } else {
            None
        }
    })
}

/// Deserializes an HTTP response into the specified type.
///
/// # Errors
///
/// Returns `Error::Request` if reading the response fails.
/// Returns `Error::Deserialize` if parsing the JSON fails.
async fn deserialize_response<T: DeserializeOwned>(response: Response) -> Result<T, Error> {
    let text = response.text().await.map_err(Error::Request)?;
    let deserializer = &mut serde_json::Deserializer::from_slice(text.as_bytes());

    serde_path_to_error::deserialize(deserializer)
        .inspect_err(|err| error!(?err, body = %text, "failed to parse json response"))
        .map_err(Error::Deserialize)
}
