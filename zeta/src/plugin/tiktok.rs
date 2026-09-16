//! TikTok integration.
//!
//! Summarises TikTok video links using TikTok's oEmbed API and, if mirroring is configured,
//! downloads the videos with `yt-dlp` and mirrors them to an S3-compatible bucket, replying with a
//! public link to the mirrored file.
//!
//! Mirroring is configured through the top-level `[mirror]` configuration section (or the `S3_*`
//! environment variables); without it, the plugin only posts video summaries.

mod oembed;
mod urls;

use std::fmt::Write;

use serde::{Deserialize, Serialize};
use tracing::{debug, error};
use url::Url;

use crate::{
    http,
    mirror::{Mirror, MirrorTarget},
    plugin::{self, prelude::*},
    utils::Truncatable,
};

use self::oembed::OEmbed;
use self::urls::{TiktokLink, parse_tiktok_url, short_url, video_url};

/// The default public URL that mirrored videos are linked with.
const DEFAULT_PUBLIC_URL_BASE: &str = "https://pub.rwx.im/tiktok";

/// Settings for the tiktok plugin, from its `[plugins.tiktok]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The maximum length of a video title before it gets truncated.
    #[serde(default = "default_title_length")]
    pub title_length: usize,
    /// The key prefix that mirrored videos are uploaded under.
    ///
    /// Falls back to the `TIKTOK_S3_PREFIX` environment variable, and to `tiktok` when neither is
    /// set.
    #[serde(default)]
    pub prefix: Option<String>,
    /// The base URL used when linking to mirrored videos.
    ///
    /// Falls back to the `TIKTOK_PUBLIC_URL_BASE` environment variable when unset. Links are
    /// built by appending the video id as a URL fragment, so the base must point at a viewer page
    /// that resolves the fragment — not directly at the bucket.
    #[serde(default)]
    pub public_url_base: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            title_length: default_title_length(),
            prefix: None,
            public_url_base: None,
        }
    }
}

/// Returns the default maximum title length.
const fn default_title_length() -> usize {
    150
}

pub struct Tiktok {
    client: reqwest::Client,
    mirror: Option<MirrorTarget>,
    settings: Settings,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("shortened link did not redirect to a valid url")]
    InvalidRedirect,
    #[error("oembed error: {0}")]
    OEmbed(#[from] oembed::Error),
}

#[async_trait]
impl Plugin<Context> for Tiktok {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings) -> Result<Tiktok, ZetaError> {
        let mirror = MirrorTarget::resolve(
            ctx.shared.get::<Mirror>(),
            settings.prefix.as_deref(),
            "TIKTOK_S3_PREFIX",
            "tiktok",
            settings.public_url_base.as_deref(),
            "TIKTOK_PUBLIC_URL_BASE",
            DEFAULT_PUBLIC_URL_BASE,
        );

        Ok(Tiktok {
            client: http::build_client(&ctx.config.http),
            mirror,
            settings: settings.clone(),
        })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "tiktok".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn url_hosts(&self) -> &'static [&'static str] {
        &["tiktok.com", "vm.tiktok.com", "www.tiktok.com"]
    }

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        if let Some(mirror) = &self.mirror {
            mirror.start_downloads();
        }

        Ok(())
    }

    async fn handle_message(
        &self,
        ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(urls) = plugin::extract_urls(user_message)
        {
            let filters = Filters::from_context(ctx);
            let sender = Sender::from_message(message);
            let urls: Vec<_> = urls
                .into_iter()
                .filter(|url| !filters.is_filtered(channel, sender, url))
                .collect();

            if let Err(err) = self.process_urls(&urls, channel, client).await {
                error!("could not process urls: {err}");
            }
        }

        Ok(())
    }
}

impl Tiktok {
    async fn process_urls(
        &self,
        urls: &[Url],
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        for url in urls {
            self.process_url(url, channel, client).await?;
        }

        Ok(())
    }

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
            let _ = client.send_privmsg(channel, formatted(&summary));
        }

        self.mirror_video(&url, video_id, channel, client).await;

        Ok(())
    }

    /// Starts mirroring of the given video, replying with a link to the mirrored file.
    ///
    /// If the video has already been mirrored, the existing link is sent immediately; otherwise
    /// the download and upload happens in a background task that replies with the link.
    async fn mirror_video(&self, url: &str, video_id: &str, channel: &str, client: &Client) {
        let Some(mirror) = &self.mirror else {
            return;
        };

        let sender = client.sender();

        let on_mirrored = {
            let channel = channel.to_string();
            move |link: String| {
                let _ = sender.send_privmsg(&channel, formatted(&link));
            }
        };

        match mirror.ensure_mirrored(url, video_id, on_mirrored).await {
            Ok(Some(link)) => {
                let _ = client.send_privmsg(channel, formatted(&link));
            }
            Ok(None) => {}
            Err(err) => {
                error!(%video_id, error = %err, "could not check if the video is already mirrored");
            }
        }
    }

    /// Requests the redirect with the given id and returns the location it redirects to.
    async fn resolve_redirect_url(&self, id: &str) -> Result<Url, Error> {
        debug!(%id, "fetching redirect url");
        let response = self.client.get(short_url(id)).send().await?;

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
fn format_summary(embed: &OEmbed, title_length: usize) -> Option<String> {
    let mut buf = String::new();

    if let Some(title) = embed.title.as_deref() {
        let truncated = title.truncate_with_suffix(title_length, "…");
        let trimmed = truncated.trim();

        let _ = write!(buf, "“\x0f{trimmed}\x0310” is a ");
    }

    if let Some(author_name) = embed.author_name.as_deref() {
        let _ = write!(buf, "TikTok video by\x0f {author_name}");
    }

    (!buf.is_empty()).then_some(buf)
}

fn formatted(s: &str) -> String {
    format!("\x0310> {s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings() {
        let settings = Settings::default();

        assert_eq!(settings.title_length, 150);
        assert!(settings.prefix.is_none());
        assert!(settings.public_url_base.is_none());
    }

    #[test]
    fn settings_deserialize() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "title_length": 100,
            "prefix": "~meta/tiktok",
            "public_url_base": "https://pub.example.com/tiktok",
        }))
        .expect("could not deserialize settings");

        assert_eq!(settings.title_length, 100);
        assert_eq!(settings.prefix.as_deref(), Some("~meta/tiktok"));
        assert_eq!(
            settings.public_url_base.as_deref(),
            Some("https://pub.example.com/tiktok")
        );
    }
}
