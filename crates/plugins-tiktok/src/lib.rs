//! Summarises TikTok video links and mirrors them to S3.
//!
//! Video details are fetched from TikTok's oEmbed API and, if mirroring is configured, the
//! videos are downloaded with `yt-dlp` and mirrored to an S3-compatible bucket, replying with a
//! public link to the mirrored file.
//!
//! Mirroring is configured through the top-level `[mirror]` configuration section (or the `S3_*`
//! environment variables); without it, the plugin only posts video summaries. Posting the
//! summaries and mirroring can each be turned off through this plugin's settings.

mod oembed;
mod urls;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use url::Url;

use zeta_http as http;
use zeta_http::RequestError;
use zeta_mirror::{MediaId, Mirror, MirrorHandle};
use zeta_plugin::prelude::*;
use zeta_url::RedactedUrl;
use zeta_utils::irc::unstyle_text;
use zeta_utils::text::Truncatable;

use self::oembed::OEmbed;
use self::urls::{TiktokLink, parse_tiktok_url, short_url, video_url};

/// Settings for the tiktok plugin, from its `[plugins.tiktok]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Whether to post a summary of the video's oEmbed metadata.
    pub post_summary: bool,
    /// The maximum length of a video title before it gets truncated.
    pub title_length: usize,
    /// Whether videos are mirrored to the S3 bucket.
    ///
    /// Mirroring additionally requires the shared `[mirror]` configuration (or the `S3_*`
    /// environment variables); without it, videos are not mirrored regardless of this setting.
    pub mirror: bool,
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
            post_summary: true,
            title_length: 150,
            mirror: true,
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

#[plugin(name = "tiktok", settings = Settings, urls(hosts = urls::URL_HOSTS))]
impl Tiktok {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let mirror = if settings.mirror {
            MirrorHandle::resolve(
                ctx.shared.get::<Mirror>(),
                "tiktok",
                settings.prefix.as_deref(),
                settings.public_url_base.as_deref(),
            )
        } else {
            None
        };

        Ok(Self {
            client: ctx.http().clone(),
            mirror,
            settings: settings.clone(),
        })
    }

    #[handler(url)]
    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &Client,
        url: &UrlEvent,
    ) -> Result<(), ZetaError> {
        if let Err(err) = self.process_url(url.url(), url.channel(), client).await {
            warn!(exception.message = %err, "could not process url");
        }

        Ok(())
    }
}

impl Tiktok {
    async fn process_url(&self, url: &Url, channel: &str, client: &Client) -> Result<(), Error> {
        match parse_tiktok_url(url) {
            Some(TiktokLink::Video { channel: slug, id }) => {
                debug!(media.id = %id, "processing video");

                self.process_video_url(&slug, &id, channel, client).await?;
            }
            Some(TiktokLink::Shortened(short_id)) => {
                debug!(tiktok.short.id = %short_id, "resolving url for shortened url");

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
        let url = video_url(slug, video_id);

        if !self.settings.post_summary && self.mirror.is_none() {
            debug!(media.id = %video_id, "both summarizing and mirroring are disabled");
            return Ok(());
        }

        debug!(media.id = %video_id, "fetching video details");
        let embed = oembed::fetch(&self.client, &url).await?;

        if embed.is_privacy_restricted() {
            return Ok(());
        }

        if self.settings.post_summary
            && let Some(summary) = format_summary(&embed, self.settings.title_length)
        {
            let _ = client.send_privmsg(channel, plain_reply(&summary));
        }

        let Some(mirror) = self.mirror.as_ref() else {
            return Ok(());
        };

        let Ok(id) = MediaId::parse(video_id) else {
            warn!(media.id = %video_id, "ignoring mirror request with an unsafe id");

            return Ok(());
        };

        plugins_shared::mirror::ensure_mirrored_if_configured(
            Some(mirror),
            &url,
            &id,
            channel,
            client,
        )
        .await;

        Ok(())
    }

    /// Requests the redirect with the given id and returns the location it redirects to.
    async fn resolve_redirect_url(&self, id: &str) -> Result<Url, Error> {
        debug!(tiktok.short.id = %id, "fetching redirect url");
        let response = http::send(self.client.get(short_url(id))).await?;

        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(Error::InvalidRedirect)?;

        let url = Url::parse(location).map_err(|_| Error::InvalidRedirect)?;
        debug!(url.full = %RedactedUrl::from_url(&url), "fetched redirect url");
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
        .map(unstyle_text)
        .filter(|title| !title.is_empty())
        .map(|title| {
            title
                .truncate_with_suffix(title_length, "…")
                .trim()
                .to_owned()
        });
    let author = embed
        .author_name
        .as_deref()
        .map(unstyle_text)
        .filter(|author| !author.is_empty());

    match (title, author.as_deref()) {
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
    use zeta_test_support::settings_tests;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.post_summary);
            assert_eq!(settings.title_length, 150);
            assert!(settings.mirror);
            assert!(settings.prefix.is_none());
            assert!(settings.public_url_base.is_none());
        }
        deserialize: {
            "post_summary": false,
            "title_length": 100,
            "mirror": false,
            "prefix": "~meta/tiktok",
            "public_url_base": "https://pub.example.com/tiktok",
        } assert: {
            assert!(!settings.post_summary);
            assert_eq!(settings.title_length, 100);
            assert!(!settings.mirror);
            assert_eq!(settings.prefix.as_deref(), Some("~meta/tiktok"));
            assert_eq!(
                settings.public_url_base.as_deref(),
                Some("https://pub.example.com/tiktok")
            );
        }
    }

    /// Builds an oEmbed response with the given title and author.
    fn oembed(title: Option<&str>, author: Option<&str>) -> OEmbed {
        OEmbed {
            r#type: "video".to_string(),
            version: "1.0".to_string(),
            title: title.map(str::to_string),
            author_name: author.map(str::to_string),
            author_url: None,
            provider_name: None,
            provider_url: None,
            cache_age: None,
            thumbnail_url: None,
            thumbnail_width: None,
            thumbnail_height: None,
        }
    }

    #[test]
    fn summaries_strip_formatting_and_collapse_whitespace() {
        let embed = oembed(Some("\x0310Fake\nTitle\x0f"), Some("author\x02"));
        let summary = format_summary(&embed, 150).unwrap();
        let summary = unstyle_text(&summary);

        assert_eq!(summary, "“Fake Title” is a TikTok video by author");
    }
}
