//! Summarises reddit links and mirrors their hosted videos to S3.
//!
//! Link details are fetched from the Reddit API and, if mirroring is configured, the hosted
//! videos of linked submissions are downloaded with `yt-dlp` and mirrored to an S3-compatible
//! bucket, replying with a public link to the mirrored file.
//!
//! Mirroring is configured through the top-level `[mirror]` configuration section (or the `S3_*`
//! environment variables); without it, the plugin only posts summaries.

use reddit::{Link, Submission, Subreddit, URL_HOSTS};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use tracing::error;

use zeta_mirror::{Mirror, MirrorHandle};
use zeta_plugin::prelude::*;
use zeta_utils::irc::sanitize_external;
use zeta_utils::text::Truncatable;

/// Identifying HTTP user agent for API requests (i.e. `linux:zeta:<VERSION> (by /u/drizz)`)
pub const USER_AGENT: &str = concat!("linux:zeta:", env!("CARGO_PKG_VERSION"), " (by /u/drizz)");

/// Settings for the reddit plugin, from its `[plugins.reddit]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The Reddit application client id.
    ///
    /// Falls back to the `REDDIT_CLIENT_ID` environment variable when unset.
    pub client_id: Option<String>,
    /// The Reddit application client secret.
    ///
    /// Falls back to the `REDDIT_CLIENT_SECRET` environment variable when unset.
    pub client_secret: Option<String>,
    /// The key prefix that mirrored videos are uploaded under.
    ///
    /// Falls back to the `REDDIT_S3_PREFIX` environment variable, and to `reddit` when neither is
    /// set.
    pub prefix: Option<String>,
    /// The base URL used when linking to mirrored videos.
    ///
    /// Falls back to the `REDDIT_PUBLIC_URL_BASE` environment variable when unset. Links are
    /// built by appending the submission id as a URL fragment, so the base must point at a viewer
    /// page that resolves the fragment — not directly at the bucket.
    pub public_url_base: Option<String>,
}

/// Errors that can occur during Reddit interaction
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Reddit API returned an error response.
    #[error("reddit api error: {0}")]
    Reddit(#[from] reddit::Error),
    /// An irc error occurred while sending the reply.
    #[error("irc error: {0}")]
    Irc(#[from] irc::error::Error),
}

/// Reddit integration plugin.
pub struct Reddit {
    /// Reddit API client
    client: reddit::Client,
    /// Mirror for downloading and re-hosting hosted videos.
    mirror: Option<MirrorHandle>,
}

#[plugin(name = "reddit", settings = Settings, urls(hosts = URL_HOSTS))]
impl Reddit {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client_id = resolve_secret(settings.client_id.as_deref(), "REDDIT_CLIENT_ID")?;
        let client_secret: SecretString =
            resolve_secret(settings.client_secret.as_deref(), "REDDIT_CLIENT_SECRET")?.into();
        let client = reddit::Client::with_options(
            client_id,
            client_secret,
            reddit::ClientOptions {
                user_agent: Some(USER_AGENT.to_string()),
                timeout: Some(ctx.config.http.timeout),
            },
        )
        .map_err(plugin_err)?;
        let mirror = MirrorHandle::resolve(
            ctx.shared.get::<Mirror>(),
            "reddit",
            settings.prefix.as_deref(),
            settings.public_url_base.as_deref(),
        );

        Ok(Self { client, mirror })
    }

    #[handler(loaded)]
    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        plugins_shared::mirror::start_downloads(self.mirror.as_ref());

        Ok(())
    }

    #[handler(url)]
    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &Client,
        url: &UrlEvent,
    ) -> Result<(), ZetaError> {
        if let Some(link) = reddit::classify_reddit_url(url.url()) {
            let _ = self
                .process_url(link, url.channel(), client)
                .await
                .inspect_err(|e| error!("could not process url: {e}"));
        }

        Ok(())
    }
}

impl Reddit {
    /// Processes a reddit link, posting a summary for every recognized link kind.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if a Reddit API request fails or a reply cannot be sent;
    /// unrecognized links are skipped.
    async fn process_url(&self, link: Link, channel: &str, client: &Client) -> Result<(), Error> {
        match link {
            Link::Gallery(id) | Link::CommentsPage { id } | Link::Submission { id, .. } => {
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
                        client.send_privmsg(channel, format_subreddit(&subreddit))?;
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
        let id = submission.id.as_deref().unwrap_or(fallback_id).to_string();

        client.send_privmsg(channel, notice(format_submission(&submission)))?;

        // The media URL from the API is used for the download: reddit's anti-bot filters may
        // prevent `yt-dlp` from accessing the video through the link itself.
        if let Some(url) = submission.video_url() {
            plugins_shared::mirror::ensure_mirrored_if_configured(
                self.mirror.as_ref(),
                url,
                &id,
                channel,
                client,
            )
            .await;
        }

        Ok(())
    }
}

/// Formats a submission's title and subreddit as an IRC notice, sanitized for a single IRC line.
fn format_submission(submission: &Submission) -> String {
    notice(format!(
        "{} : {}",
        sanitize_external(&submission.title),
        sanitize_external(&submission.subreddit)
    ))
}

/// Formats a subreddit's title and public description as an IRC reply, sanitized for a single
/// IRC line.
///
/// The description is sanitized before it is truncated, so the character budget counts what is
/// actually sent.
fn format_subreddit(subreddit: &Subreddit) -> String {
    let title = sanitize_external(&subreddit.title);
    let description = sanitize_external(&subreddit.public_description);
    let description = description.truncate_with_suffix(250, "…");

    reply(&title, description)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;
    use zeta_utils::irc::strip_formatting;

    /// Builds a submission from the API shape used in the tests below.
    fn submission(title: &str, subreddit: &str) -> Submission {
        serde_json::from_value(serde_json::json!({
            "subreddit": subreddit,
            "title": title,
            "ups": 1,
            "upvote_ratio": 1.0,
            "selftext": "",
            "url": "https://example.com/",
        }))
        .expect("the test submission should deserialize")
    }

    /// Builds a subreddit from the API shape used in the tests below.
    fn subreddit(title: &str, public_description: &str) -> Subreddit {
        serde_json::from_value(serde_json::json!({
            "display_name": "denmark",
            "title": title,
            "public_description": public_description,
            "subscribers": 1,
            "url": "/r/denmark/",
        }))
        .expect("the test subreddit should deserialize")
    }

    #[test]
    fn formats_submission_as_a_notice() {
        let submission = submission("Sparindex S&amp;P500 børsnoteres i morgen", "dkfinance");

        assert_eq!(
            strip_formatting(&format_submission(&submission)),
            "> Sparindex S&P500 børsnoteres i morgen : dkfinance"
        );
    }

    #[test]
    fn formats_subreddit_as_a_reply() {
        let subreddit = subreddit("r/Denmark &amp; friends", "News &amp; discussion");

        assert_eq!(
            strip_formatting(&format_subreddit(&subreddit)),
            "> r/Denmark & friends: News & discussion"
        );
    }

    #[test]
    fn sanitizes_control_characters_in_replies() {
        let submission = submission("Fake\x0310\nTitle", "dkfinance");
        let subreddit = subreddit("r/Denmark", "News&#10;and\x02 discussion");

        assert_eq!(
            strip_formatting(&format_submission(&submission)),
            "> Fake Title : dkfinance"
        );
        assert_eq!(
            strip_formatting(&format_subreddit(&subreddit)),
            "> r/Denmark: News and discussion"
        );
    }

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.client_id.is_none());
            assert!(settings.client_secret.is_none());
            assert!(settings.prefix.is_none());
            assert!(settings.public_url_base.is_none());
        }
        deserialize: {
            "client_id": "id",
            "client_secret": "secret",
            "prefix": "~meta/reddit",
            "public_url_base": "https://pub.example.com/reddit",
        } assert: {
            assert_eq!(settings.client_id.as_deref(), Some("id"));
            assert_eq!(settings.client_secret.as_deref(), Some("secret"));
            assert_eq!(settings.prefix.as_deref(), Some("~meta/reddit"));
            assert_eq!(
                settings.public_url_base.as_deref(),
                Some("https://pub.example.com/reddit")
            );
        }
    }
}
