//! Reddit integration.
//!
//! Summarises reddit links using the Reddit API and, if mirroring is configured, downloads the
//! hosted videos of linked submissions with `yt-dlp` and mirrors them to an S3-compatible bucket,
//! replying with a public link to the mirrored file.
//!
//! Mirroring is configured through the top-level `[mirror]` configuration section (or the `S3_*`
//! environment variables); without it, the plugin only posts summaries.

use reddit::Link;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use tracing::error;
use url::Url;

use crate::{
    mirror::{Mirror, MirrorTarget},
    plugin::prelude::*,
    utils::Truncatable,
};

/// The reddit API submission model.
use reddit::Submission;

/// Identifying HTTP user agent for API requests (i.e. `linux:zeta:<VERSION> (by /u/drizz)`)
pub const USER_AGENT: &str = concat!("linux:zeta:", env!("CARGO_PKG_VERSION"), " (by /u/drizz)");

/// The default public URL that mirrored videos are linked with.
const DEFAULT_PUBLIC_URL_BASE: &str = "https://pub.rwx.im/reddit";

/// Settings for the reddit plugin, from its `[plugins.reddit]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The Reddit application client id.
    ///
    /// Falls back to the `REDDIT_CLIENT_ID` environment variable when unset.
    #[serde(default)]
    pub client_id: Option<String>,
    /// The Reddit application client secret.
    ///
    /// Falls back to the `REDDIT_CLIENT_SECRET` environment variable when unset.
    #[serde(default)]
    pub client_secret: Option<String>,
    /// The key prefix that mirrored videos are uploaded under.
    ///
    /// Falls back to the `REDDIT_S3_PREFIX` environment variable, and to `reddit` when neither is
    /// set.
    #[serde(default)]
    pub prefix: Option<String>,
    /// The base URL used when linking to mirrored videos.
    ///
    /// Falls back to the `REDDIT_PUBLIC_URL_BASE` environment variable when unset. Links are
    /// built by appending the submission id as a URL fragment, so the base must point at a viewer
    /// page that resolves the fragment — not directly at the bucket.
    #[serde(default)]
    pub public_url_base: Option<String>,
}

/// Errors that can occur during Reddit interaction
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("reddit api error: {0}")]
    Reddit(#[from] reddit::Error),
    #[error("irc error: {0}")]
    Irc(#[from] irc::error::Error),
}

/// Reddit integration plugin.
pub struct Reddit {
    /// Reddit API client
    client: reddit::Client,
    /// Mirror for downloading and re-hosting hosted videos.
    mirror: Option<MirrorTarget>,
}

#[async_trait]
impl Plugin<Context> for Reddit {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client_id = resolve_secret(settings.client_id.as_deref(), "REDDIT_CLIENT_ID")?;
        let client_secret: SecretString =
            resolve_secret(settings.client_secret.as_deref(), "REDDIT_CLIENT_SECRET")?.into();
        let user_agent = Some(USER_AGENT.to_string());
        let timeout = Some(ctx.config.http.timeout);
        let client = reddit::Client::new(client_id, client_secret, user_agent, timeout);
        let mirror = MirrorTarget::resolve(
            ctx.shared.get::<Mirror>(),
            settings.prefix.as_deref(),
            "REDDIT_S3_PREFIX",
            "reddit",
            settings.public_url_base.as_deref(),
            "REDDIT_PUBLIC_URL_BASE",
            DEFAULT_PUBLIC_URL_BASE,
        );

        Ok(Reddit { client, mirror })
    }

    fn url_hosts(&self) -> &'static [&'static str] {
        &[
            "i.redd.it",
            "oauth.reddit.com",
            "old.reddit.com",
            "preview.redd.it",
            "redd.it",
            "reddit.com",
            "v.redd.it",
            "www.reddit.com",
        ]
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
        if let Some(urls) = FilteredUrls::from_message(ctx, message) {
            let channel = urls.channel();
            let urls: Vec<_> = urls.collect();

            let _ = self
                .process_urls(&urls, channel, client)
                .await
                .inspect_err(|e| error!("error when processing urls: {e}"));
        }

        Ok(())
    }
}

impl Reddit {
    pub async fn process_urls(
        &self,
        urls: &Vec<Url>,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        for url in urls {
            if let Some(link) = reddit::classify_reddit_url(url) {
                self.process_url(link, channel, client).await?;
            }
        }

        Ok(())
    }

    async fn process_url(&self, link: Link, channel: &str, client: &Client) -> Result<(), Error> {
        match link {
            Link::Gallery(id) | Link::Comments { id } | Link::Submission { id, .. } => {
                self.process_submission(&id, channel, client).await?;
            }
            Link::Comment { submission, .. } => {
                self.process_submission(&submission, channel, client)
                    .await?;
            }
            Link::Video(id) => match self.client.video(&id).await {
                Ok(submission) => {
                    self.process_fetched_submission(submission, &id, channel, client)
                        .await?;
                }
                Err(err) => {
                    client.send_privmsg(
                        channel,
                        notice(format!("could not resolve video link: {err}")),
                    )?;
                }
            },
            Link::Shortened { id, subreddit } => {
                match self.client.resolve_shortened_link(&subreddit, &id).await {
                    Ok(link) => {
                        if let Err(e) = Box::pin(self.process_url(link, channel, client)).await {
                            error!("failed to process resolved link: {e}");
                        }
                    }
                    Err(err) => {
                        client.send_privmsg(
                            channel,
                            notice(format!("could not resolve shortened link: {err}")),
                        )?;
                    }
                }
            }
            Link::Subreddit(subreddit) => {
                match self.client.subreddit_about_info(&subreddit).await {
                    Ok(subreddit) => {
                        let title = subreddit.title;
                        let description =
                            subreddit.public_description.truncate_with_suffix(250, "…");

                        client.send_privmsg(channel, reply(&title, &description))?;
                    }
                    Err(err) => {
                        client.send_privmsg(
                            channel,
                            notice(format!("could not fetch subreddit details: {err}")),
                        )?;
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }

    /// Fetches the details of the submission with the given id and processes them.
    async fn process_submission(
        &self,
        id: &str,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        match self.client.submission(id).await {
            Ok(submission) => {
                self.process_fetched_submission(submission, id, channel, client)
                    .await?;
            }
            Err(err) => {
                client.send_privmsg(
                    channel,
                    notice(format!("could not fetch submission details: {err}")),
                )?;
            }
        }

        Ok(())
    }

    /// Posts a summary of the submission and mirrors its hosted video, if any.
    async fn process_fetched_submission(
        &self,
        submission: Submission,
        fallback_id: &str,
        channel: &str,
        client: &Client,
    ) -> Result<(), Error> {
        let title = submission.title.clone();
        let subreddit = submission.subreddit.clone();
        let id = submission.id.as_deref().unwrap_or(fallback_id).to_string();

        client.send_privmsg(channel, notice(format!("{title} : {subreddit}")))?;

        self.mirror_video(&submission, &id, channel, client).await;

        Ok(())
    }

    /// Starts mirroring of the hosted video of the submission, replying with a link to the
    /// mirrored file.
    ///
    /// If the video has already been mirrored, the existing link is sent immediately; otherwise
    /// the download and upload happens in a background task that replies with the link.
    async fn mirror_video(
        &self,
        submission: &Submission,
        id: &str,
        channel: &str,
        client: &Client,
    ) {
        let Some(mirror) = &self.mirror else {
            return;
        };

        // The media URL from the API is used for the download: reddit's anti-bot filters may
        // prevent `yt-dlp` from accessing the video through the link itself.
        let Some(url) = submission.video_url() else {
            return;
        };

        let sender = client.sender();

        let on_mirrored = {
            let channel = channel.to_string();
            move |link: String| {
                let _ = sender.send_privmsg(&channel, notice(link));
            }
        };

        match mirror.ensure_mirrored(url, id, on_mirrored).await {
            Ok(Some(link)) => {
                let _ = client.send_privmsg(channel, notice(link));
            }
            Ok(None) => {}
            Err(err) => {
                error!(%id, error = %err, "could not check if the video is already mirrored");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings() {
        let settings = Settings::default();

        assert!(settings.client_id.is_none());
        assert!(settings.client_secret.is_none());
        assert!(settings.prefix.is_none());
        assert!(settings.public_url_base.is_none());
    }

    #[test]
    fn settings_deserialize() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "client_id": "id",
            "client_secret": "secret",
            "prefix": "~meta/reddit",
            "public_url_base": "https://pub.example.com/reddit",
        }))
        .expect("could not deserialize settings");

        assert_eq!(settings.client_id.as_deref(), Some("id"));
        assert_eq!(settings.client_secret.as_deref(), Some("secret"));
        assert_eq!(settings.prefix.as_deref(), Some("~meta/reddit"));
        assert_eq!(
            settings.public_url_base.as_deref(),
            Some("https://pub.example.com/reddit")
        );
    }
}
