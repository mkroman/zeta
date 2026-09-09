//! TikTok's oEmbed API.

use serde::Deserialize;
use tracing::debug;

/// The URL to the oEmbed endpoint.
const TIKTOK_OEMBED_API: &str = "https://www.tiktok.com/oembed";

/// The oEmbed response for a TikTok video.
#[derive(Debug, Eq, PartialEq, Deserialize)]
pub struct OEmbed {
    /// The resource type.
    pub r#type: String,
    /// The oEmbed version number.
    pub version: String,
    /// A text title, describing the resource.
    pub title: Option<String>,
    /// The name of the author/owner of the resource.
    pub author_name: Option<String>,
    /// A URL for the author/owner of the resource.
    pub author_url: Option<String>,
    /// The name of the resource provider.
    pub provider_name: Option<String>,
    /// The URL for the resource provider.
    pub provider_url: Option<String>,
    /// The suggested cache lifetime for this resource, in seconds. Consumers may choose to use this
    /// value or not.
    pub cache_age: Option<u32>,
    /// A URL to a thumbnail image representing the resource.
    pub thumbnail_url: Option<String>,
    /// The width of the optional thumbnail.
    pub thumbnail_width: Option<u32>,
    /// The height of the optional thumbnail.
    pub thumbnail_height: Option<u32>,
}

impl OEmbed {
    /// Returns whether the response indicates a privacy-restricted video.
    ///
    /// The oEmbed endpoint returns these placeholder author values when the video is private, in
    /// which case the video should be ignored.
    #[must_use]
    pub fn is_privacy_restricted(&self) -> bool {
        self.author_name.as_deref() == Some("@")
            && self.author_url.as_deref() == Some("https://www.tiktok.com/")
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("tiktok returned invalid oembed response")]
    InvalidResponse,
}

/// Fetches the oEmbed details for the given video `url`.
///
/// # Errors
///
/// Returns an error if the request fails or TikTok returns an invalid response.
pub async fn fetch(http_client: &reqwest::Client, url: &str) -> Result<OEmbed, Error> {
    debug!(%url, "fetching oembed data");
    let request = http_client.get(TIKTOK_OEMBED_API).query(&[("url", url)]);
    let response = request.send().await.map_err(Error::Request)?;
    let oembed = response.json().await.map_err(|_| Error::InvalidResponse)?;

    Ok(oembed)
}
