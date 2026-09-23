//! Summarises TikTok video links and mirrors them to S3.
//!
//! Video details are fetched from TikTok's oEmbed API and, if mirroring is configured, the
//! videos are downloaded with `yt-dlp` and mirrored to an S3-compatible bucket, replying with a
//! public link to the mirrored file.
//!
//! Mirroring is configured through the top-level `[mirror]` configuration section (or the `S3_*`
//! environment variables); without it, the plugin only posts video summaries.

mod oembed;
mod urls;


use serde::{Deserialize, Serialize};
use tracing::{debug, error};
use url::Url;

use crate::{
    error::RequestError,
    http,
    mirror::{Mirror, MirrorHandle},
    plugin::prelude::*,
    utils::Truncatable,
};

use self::oembed::OEmbed;
use self::urls::{TiktokLink, parse_tiktok_url, short_url, video_url};

/// Settings for the tiktok plugin, from its `[plugins.tiktok]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The maximum length of a video title before it gets truncated.
    pub title_length: usize,
    /// The key prefix that mirrored videos are uploaded under.
    ///
    /// Falls back to the `TIKTOK_S3_PREFIX` environment variable, and to `tiktok` when neither is
    /// set.
    pub prefix: Option<String>,
    /// The base URL used when linking to mirrored videos.
    ///
    /// Falls back to the `TIKTOK_PUBLIC_URL_BASE` environment variable when unset. Links are
    /// built by appending the video id as a URL fragment, so the base must point at a viewer page
    /// that resolves the fragment — not directly at the bucket.
    pub public_url_base: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            title_length: 150,
            prefix: None,
            public_url_base: None,
        }
    }
}

/// The TikTok plugin: summarizes and mirrors TikTok video links.
pub struct Tiktok {
    client: reqwest::Client,
    mirror: Option<MirrorHandle>,
    settings: Settings,
}

/// Errors that can occur while summarizing or mirroring TikTok videos.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// Sending the HTTP request failed.
    #[error("request error: {0}")]
    Request(#[from] RequestError),
    /// A shortened link did not redirect to a valid TikTok URL.
    #[error("shortened link did not redirect to a valid url")]
    InvalidRedirect,
    /// TikTok's oEmbed API returned an error response.
    #[error("oembed error: {0}")]
    OEmbed(#[from] oembed::Error),
}

#[async_trait]
impl Plugin<Context> for Tiktok {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Tiktok, ZetaError> {
        subscriptions.urls(UrlScope::Hosts(urls::URL_HOSTS));

        let mirror = MirrorHandle::resolve(
            ctx.shared.get::<Mirror>(),
            "tiktok",
            settings.prefix.as_deref(),
            settings.public_url_base.as_deref(),
        );

        Ok(Tiktok {
            client: http::build_client(&ctx.config.http),
            mirror,
            settings: settings.clone(),
        })
    }

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        if let Some(mirror) = &self.mirror {
            mirror.start_downloads();
        }

        Ok(())
    }

    async fn handle_url(&self, _ctx: &Context, client: &Client, url: &UrlEvent) -> Result<(), ZetaError> {
        if let Err(err) = self.process_url(url.url(), url.channel(), client).await {
            error!("could not process url: {err}");
        }

        Ok(())
    }
}

impl Tiktok {
    async fn process_url(&self, url: &Url, channel: &str, client: &Client) -> Result<(), Error> {
        match parse_tiktok_url(url) {
            Some(TiktokLink::Video { channel: slug, id }) => {
                debug!(video_id = %id, "processing video");

                self.process_video_url(&slug, &id, channel, client).await?;
            }
            Some(TiktokLink::Shortened(short_id)) => {
                debug!(%short_id, "resolving url for shortened url");

                let resolved_url = self.resolve_redirect_url(&short_id).await?;

                if let Some(TiktokLink::Video { channel: slug, id }) =
                    parse_tiktok_url(&resolved_url)
                {
                    self.process_video_url(&slug, &id, channel, client).await?;
                }
            }
            _ => {}
        }

        Ok(())
    }

    async fn process_video_url(
        &self,
        slug: &str,
        video_id: &str,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        debug!(%video_id, "fetching video details");

        let url = video_url(slug, video_id);
        let embed = oembed::fetch(&self.client, &url).await?;

        if embed.is_privacy_restricted() {
            return Ok(());
        }

        if let Some(summary) = format_summary(&embed, self.settings.title_length) {
            let _ = client.send_privmsg(channel, notice(&summary));
        }

        self.mirror_video(&url, video_id, channel, client).await;

        Ok(())
    }

    /// Starts mirroring of the given video, replying with a link to the mirrored file.
    ///
    /// If the video has already been mirrored, the existing link is sent immediately; otherwise
    /// the download and upload happens in a background task that replies with the link.
    async fn mirror_video(&self, url: &str, video_id: &str, channel: &str, client: &Client) {
        if let Some(mirror) = &self.mirror {
            mirror.ensure_mirrored_and_reply(url, video_id, channel, client).await;
        }
    }

    /// Requests the redirect with the given id and returns the location it redirects to.
    async fn resolve_redirect_url(&self, id: &str) -> Result<Url, Error> {
        debug!(%id, "fetching redirect url");
        let response = http::send(self.client.get(short_url(id))).await?;

        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(Error::InvalidRedirect)?;

        let url = Url::parse(location).map_err(|_| Error::InvalidRedirect)?;
        debug!(%url, "fetched redirect url");
        debug_assert_eq!(url.host_str(), Some("www.tiktok.com"));

        Ok(url)
    }
}

/// Formats the oEmbed details as a human-readable summary of the video.
///
/// The title and author clauses are written independently, so a summary with only one of them
/// does not carry the other's connecting text.
fn format_summary(embed: &OEmbed, title_length: usize) -> Option<String> {
    let title = embed
        .title
        .as_deref()
        .map(|title| title.truncate_with_suffix(title_length, "…").trim().to_owned());

    match (title, embed.author_name.as_deref()) {
        (Some(title), Some(author)) => Some(format!(
            "“{RESET}{title}{COLOR}” is a TikTok video by{RESET} {author}"
        )),
        (Some(title), None) => Some(format!("“{RESET}{title}{COLOR}”")),
        (None, Some(author)) => Some(format!("TikTok video by{RESET} {author}")),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert_eq!(settings.title_length, 150);
            assert!(settings.prefix.is_none());
            assert!(settings.public_url_base.is_none());
        }
        deserialize: {
            "title_length": 100,
            "prefix": "~meta/tiktok",
            "public_url_base": "https://pub.example.com/tiktok",
        } assert: {
            assert_eq!(settings.title_length, 100);
            assert_eq!(settings.prefix.as_deref(), Some("~meta/tiktok"));
            assert_eq!(
                settings.public_url_base.as_deref(),
                Some("https://pub.example.com/tiktok")
            );
        }
    }
}
