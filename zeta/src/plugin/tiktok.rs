//! TikTok integration.
//!
//! Summarises TikTok video links using TikTok's oEmbed API and, if mirroring is configured,
//! downloads the videos with `yt-dlp` and mirrors them to an S3-compatible bucket, replying with a
//! public link to the mirrored file.
//!
//! Mirroring is configured through the `S3_*` environment variables (see `s3::S3::from_env`);
//! without it, the plugin only posts video summaries.

mod manager;
mod mirror;
mod oembed;
mod s3;
mod urls;
mod ytdlp;

use std::fmt::Write;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, error, warn};
use url::Url;

use crate::{
    http,
    plugin::{self, prelude::*},
    utils::Truncatable,
};

use self::mirror::Mirror;
use self::oembed::OEmbed;
use self::urls::{parse_tiktok_url, short_url, video_url, TiktokLink};
use self::ytdlp::{YtDlp, YtDlpOptions};

/// Settings for the tiktok plugin, from its `[plugins.tiktok]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The maximum length of a video title before it gets truncated.
    #[serde(default = "default_title_length")]
    pub title_length: usize,
    /// The maximum size of a video to download, as passed to `yt-dlp`.
    ///
    /// Videos that report a larger size upfront are skipped.
    #[serde(default = "default_max_filesize")]
    pub max_filesize: String,
    /// The maximum duration of a download before it gets killed.
    #[serde(default = "default_download_timeout", with = "humantime_serde")]
    pub download_timeout: Duration,
    /// The maximum number of downloads that run concurrently.
    #[serde(default = "default_max_concurrent_downloads")]
    pub max_concurrent_downloads: usize,
    /// The command used to run `yt-dlp`.
    ///
    /// Falls back to the `TIKTOK_YTDLP_COMMAND` environment variable, and to `yt-dlp` when
    /// neither is set.
    #[serde(default)]
    pub ytdlp_command: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            title_length: default_title_length(),
            max_filesize: default_max_filesize(),
            download_timeout: default_download_timeout(),
            max_concurrent_downloads: default_max_concurrent_downloads(),
            ytdlp_command: None,
        }
    }
}

/// Returns the default maximum title length.
const fn default_title_length() -> usize {
    150
}

/// Returns the default maximum video size.
fn default_max_filesize() -> String {
    "500M".to_string()
}

/// Returns the default download timeout.
const fn default_download_timeout() -> Duration {
    Duration::from_mins(10)
}

/// Returns the default maximum number of concurrent downloads.
const fn default_max_concurrent_downloads() -> usize {
    2
}

pub struct Tiktok {
    client: reqwest::Client,
    mirror: Option<Mirror>,
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
    fn new(ctx: &Context) -> Result<Tiktok, ZetaError> {
        let settings = ctx.config.plugins.tiktok.settings.clone();
        let ytdlp = YtDlp::new(YtDlpOptions {
            command: settings.ytdlp_command.clone(),
            max_filesize: settings.max_filesize.clone(),
            download_timeout: settings.download_timeout,
        });
        let mirror = match Mirror::from_env(ytdlp, settings.max_concurrent_downloads) {
            Ok(mirror) => Some(mirror),
            Err(err) => {
                warn!(error = %err, "tiktok mirroring is disabled");
                None
            }
        };

        Ok(Tiktok {
            client: http::build_client(&ctx.config.http),
            mirror,
            settings,
        })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "tiktok".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        if let Some(mirror) = &mut self.mirror {
            mirror.start_downloads();
        }

        Ok(())
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(urls) = plugin::extract_urls(user_message)
            && let Err(err) = self.process_urls(&urls, channel, client).await
        {
            error!("could not process urls: {err}");
        }

        Ok(())
    }
}

impl Tiktok {
    async fn process_urls(&self, urls: &[Url], channel: &str, client: &Client) -> Result<(), Error> {
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

                if let Some(TiktokLink::Video { channel: slug, id }) = parse_tiktok_url(&resolved_url)
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

        let _ = write!(buf, "“\x0f{truncated}\x0310” is a ");
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
        assert_eq!(settings.max_filesize, "500M");
        assert_eq!(settings.download_timeout, Duration::from_mins(10));
        assert_eq!(settings.max_concurrent_downloads, 2);
        assert!(settings.ytdlp_command.is_none());
    }

    #[test]
    fn settings_deserialize() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "title_length": 100,
            "max_filesize": "100M",
            "download_timeout": "5m",
            "max_concurrent_downloads": 1,
            "ytdlp_command": "/usr/local/bin/yt-dlp",
        }))
        .expect("could not deserialize settings");

        assert_eq!(settings.title_length, 100);
        assert_eq!(settings.max_filesize, "100M");
        assert_eq!(settings.download_timeout, Duration::from_mins(5));
        assert_eq!(settings.max_concurrent_downloads, 1);
        assert_eq!(
            settings.ytdlp_command.as_deref(),
            Some("/usr/local/bin/yt-dlp")
        );
    }
}
