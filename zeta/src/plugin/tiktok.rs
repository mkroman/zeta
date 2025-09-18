use std::fmt::Write;

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use reqwest::header::LOCATION;
use serde::Deserialize;
use tracing::{debug, error};
use url::Url;

use super::{Author, Name, Plugin, Version};
use crate::utils::Truncatable;
use crate::{Error as ZetaError, plugin};

/// The URL to the oEmbed endpoint.
const TIKTOK_OEMBED_API: &str = "https://www.tiktok.com/oembed";

/// The hostname used for shortened URLs.
const TIKTOK_SHORT_HOST: &str = "vm.tiktok.com";

/// The standard hostname.
const TIKTOK_STANDARD_HOST: &str = "tiktok.com";

/// The maximum length of a TikTok videos' title before it gets truncated.
const TIKTOK_TITLE_LENGTH: usize = 150;

pub struct Tiktok {
    client: reqwest::Client,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("location header is missing from response")]
    MissingLocationHeader,
    #[error("location header contains invalid utf-8")]
    InvalidHeaderValue,
    #[error("location header contains invalid URL")]
    InvalidUrl,
    #[error("tiktok returned invalid oembed response")]
    InvalidOEmbed,
    #[error("shortened link redirects to invalid url")]
    InvalidRedirectUrl,
}

#[derive(Eq, PartialEq, Debug)]
#[non_exhaustive]
pub enum UrlKind {
    Video(String, String),
    Channel(String),
    Shortened(String),
}

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
    /// The suggested cache lifetime for this resource, in seconds. Consumers may choose to use this value or not.
    pub cache_age: Option<u32>,
    /// A URL to a thumbnail image representing the resource.
    pub thumbnail_url: Option<String>,
    /// The width of the optional thumbnail.
    pub thumbnail_width: Option<u32>,
    /// The height of the optional thumbnail.
    pub thumbnail_height: Option<u32>,
}

#[async_trait]
impl Plugin for Tiktok {
    fn new() -> Tiktok {
        Tiktok::new()
    }

    fn name() -> Name {
        Name("tiktok")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(urls) = extract_urls(user_message)
        {
            self.process_urls(urls, channel, client)
                .await
                .map_err(|e| ZetaError::PluginError(Box::new(e)))?;
        }

        Ok(())
    }
}

impl Tiktok {
    pub fn new() -> Self {
        let client = plugin::build_http_client();

        Self { client }
    }

    async fn process_urls(
        &self,
        urls: Vec<Url>,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        for url in urls {
            debug!(%url, "processing url");
            self.process_url(&url, channel, client).await?;
            debug!(%url, "finished processing url");
        }

        Ok(())
    }

    async fn process_url(&self, url: &Url, channel: &str, client: &Client) -> Result<(), Error> {
        match classify_tiktok_url(url) {
            Some(UrlKind::Video(channel_slug, video_id)) => {
                debug!(%video_id, "processing video");

                self.process_video_url(&channel_slug, &video_id, channel, client)
                    .await?;
            }
            Some(UrlKind::Shortened(short_id)) => {
                debug!(%short_id, "resolving url for shortened url");

                let resolved_url = self
                    .resolve_redirect_url(&short_id)
                    .await
                    .map_err(|_| Error::InvalidRedirectUrl)?;

                if let Some(UrlKind::Video(channel_slug, video_id)) =
                    classify_tiktok_url(&resolved_url)
                {
                    self.process_video_url(&channel_slug, &video_id, channel, client)
                        .await?;
                }
            }
            _ => {}
        }

        Ok(())
    }

    async fn process_video_url(
        &self,
        channel_slug: &str,
        video_id: &str,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        debug!(%video_id, "fetching video details");

        let url = format!("https://www.tiktok.com/{channel_slug}/video/{video_id}");
        let embed = self
            .fetch_oembed_data(url.as_str())
            .await
            .map_err(|_| Error::InvalidOEmbed)?;
        let mut buf = String::new();

        if let Some(title) = embed.title {
            let truncated = title.truncate_with_suffix(TIKTOK_TITLE_LENGTH, "…");

            let _ = write!(buf, "“\x0f{truncated}\x0310” is a ");
        }

        if let Some(author_name) = embed.author_name {
            let _ = write!(buf, "TikTok video by\x0f {author_name}");
        }

        if !buf.is_empty() {
            client.send_privmsg(channel, formatted(&buf)).unwrap();
        }

        Ok(())
    }

    async fn fetch_oembed_data(&self, url: &str) -> Result<OEmbed, Error> {
        debug!(%url, "fetching oembed data");
        let request = self.client.get(TIKTOK_OEMBED_API).query(&[("url", url)]);
        let response = request.send().await.map_err(Error::Request)?;
        let oembed = response.json().await.map_err(|_| Error::InvalidOEmbed)?;

        Ok(oembed)
    }

    /// Requests the redirect with the given id and returns the location it redirects to.
    async fn resolve_redirect_url(&self, id: &str) -> Result<Url, Error> {
        debug!(%id, "fetching redirect url");
        let request = self.client.get(format!("https://vm.tiktok.com/{id}/"));
        let response = request.send().await.map_err(Error::Request)?;
        let location = response
            .headers()
            .get(LOCATION)
            .ok_or_else(|| Error::MissingLocationHeader)?;
        let location_str = location.to_str().map_err(|_| Error::InvalidHeaderValue)?;
        let url = Url::parse(location_str).map_err(|_| Error::InvalidUrl)?;
        debug!(%url, "fetched redirect url");
        debug_assert_eq!(url.host_str(), Some("www.tiktok.com"));

        Ok(url)
    }
}

fn formatted(s: &str) -> String {
    format!("\x0310> {s}")
}

fn extract_urls(s: &str) -> Option<Vec<Url>> {
    let urls: Vec<Url> = s
        .split(' ')
        .filter(|word| word.to_ascii_lowercase().starts_with("http"))
        .filter_map(|word| Url::parse(word).ok())
        .collect();

    (!urls.is_empty()).then_some(urls)
}

/// Parses the given `url` and returns a [`UrlKind`] depending on the type of Tiktok URL.
fn classify_tiktok_url(url: &Url) -> Option<UrlKind> {
    match url.host_str()? {
        TIKTOK_STANDARD_HOST | "www.tiktok.com" => parse_tiktok_com_url(url),
        TIKTOK_SHORT_HOST => parse_shortened_tiktok_url(url),
        _ => None,
    }
}

fn parse_shortened_tiktok_url(url: &Url) -> Option<UrlKind> {
    let segments: Vec<&str> = url.path_segments()?.collect();

    match segments.as_slice() {
        [id] | [id, ""] if !id.is_empty() => Some(UrlKind::Shortened((*id).to_string())),
        _ => None,
    }
}

/// Parses tiktok.com URLs
fn parse_tiktok_com_url(url: &Url) -> Option<UrlKind> {
    let segments: Vec<&str> = url.path_segments()?.collect();

    match segments.as_slice() {
        // `/@somechannel`
        [channel] if channel.starts_with('@') && channel.len() > 1 => {
            Some(UrlKind::Channel((*channel).to_string()))
        }
        // `/@somechannel/video/7551110927479754006`
        [channel, "video", video_id] | [channel, "video", video_id, ""]
            if channel.starts_with('@') && !video_id.is_empty() =>
        {
            Some(UrlKind::Video(
                (*channel).to_string(),
                (*video_id).to_string(),
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tiktok_channel_urls() {
        let test_cases = [
            (
                "https://www.tiktok.com/@dailymail",
                Some(UrlKind::Channel("@dailymail".to_string())),
            ),
            (
                "https://tiktok.com/@user123",
                Some(UrlKind::Channel("@user123".to_string())),
            ),
            ("https://www.tiktok.com/@", None), // invalid
        ];

        for (url_str, expected) in test_cases {
            let url = Url::parse(url_str).unwrap();

            assert_eq!(classify_tiktok_url(&url), expected);
        }
    }

    #[test]
    fn test_parse_tiktok_video_urls() {
        let test_cases = [
            (
                &["https://www.tiktok.com/@dailymail/video/7541501431543532814"],
                Some(UrlKind::Video(
                    "@dailymail".to_string(),
                    "7541501431543532814".to_string(),
                )),
            ),
            (
                &["https://www.tiktok.com/@dailymail/video/7541501431543532814/"],
                Some(UrlKind::Video(
                    "@dailymail".to_string(),
                    "7541501431543532814".to_string(),
                )),
            ),
            (&["https://www.tiktok.com/@dailymail/video/"], None), // invalid
        ];

        for (url_strs, expected) in test_cases {
            for url_str in url_strs {
                let url = Url::parse(url_str).unwrap();

                assert_eq!(classify_tiktok_url(&url), expected);
            }
        }
    }

    #[test]
    fn test_parse_tiktok_short_urls() {
        let test_cases = [
            (
                &["https://vm.tiktok.com/ZNdgoKow7/"],
                Some(UrlKind::Shortened("ZNdgoKow7".to_string())),
            ),
            (&["https://vm.tiktok.com/"], None), // invalid
        ];

        for (url_strs, expected) in test_cases {
            for url_str in url_strs {
                let url = Url::parse(url_str).unwrap();

                assert_eq!(classify_tiktok_url(&url), expected);
            }
        }
    }
}
