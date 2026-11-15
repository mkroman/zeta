//! Summarises reddit links and mirrors their hosted videos to S3.
//!
//! Link details are fetched from the Reddit API and, if mirroring is configured, the hosted
//! videos of linked submissions are downloaded with `yt-dlp` and mirrored to an S3-compatible
//! bucket, replying with a public link to the mirrored file.
//!
//! Mirroring is configured through the top-level `[mirror]` configuration section (or the `S3_*`
//! environment variables); without it, the plugin only posts summaries.

use reddit::{Error as RedditError, Link, Submission, Subreddit, URL_HOSTS};
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use tracing::warn;

use zeta_mirror::{MediaId, MirrorHandle};
use zeta_plugin::irc::{labeled_reply, plain_reply, send_reply};
use zeta_plugin::{Client, Context, UrlEvent, ZetaError, plugin, plugin_err, resolve_secret};
use zeta_utils::irc;
use zeta_utils::text::TruncatableExt;

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
    /// The prefix and public URL base of mirrored videos.
    #[serde(flatten)]
    pub mirror: plugins_shared::mirror::MirrorSettings,
}

/// Errors that can occur during Reddit interaction
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The Reddit API returned an error response.
    #[error("reddit api error: {0}")]
    Reddit(#[from] reddit::Error),
    /// An irc error occurred while sending the reply.
    #[error("irc error: {0}")]
    Irc(#[from] ::irc::error::Error),
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
        let mut options = reddit::ClientOptions::default();
        options.user_agent = Some(USER_AGENT.to_string());
        options.timeout = Some(ctx.config.http.timeout);
        let client = reddit::Client::try_new_with_options(client_id, client_secret, options)
            .map_err(plugin_err)?;
        let mirror = settings.mirror.resolve(ctx, "reddit");

        Ok(Self { client, mirror })
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
                .inspect_err(|e| warn!(exception.message = %e, "could not process url"));
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
                    warn!(exception.message = %err, "could not resolve the reddit video link");

                    send_reply(client, channel, plain_reply(video_reply(&err)))?;
                }
            },
            Link::Shortened { id, subreddit } => {
                match self.client.resolve_shortened_link(&subreddit, &id).await {
                    Ok(link) => {
                        if let Err(e) = Box::pin(self.process_url(link, channel, client)).await {
                            warn!(exception.message = %e, "failed to process resolved link");
                        }
                    }
                    Err(err) => {
                        warn!(
                            exception.message = %err,
                            "could not resolve the reddit shortened link"
                        );

                        send_reply(client, channel, plain_reply(short_link_reply(&err)))?;
                    }
                }
            }
            Link::Subreddit(subreddit) => {
                match self.client.subreddit_about_info(&subreddit).await {
                    Ok(subreddit) => {
                        send_reply(client, channel, format_subreddit(&subreddit))?;
                    }
                    Err(err) => {
                        warn!(
                            exception.message = %err,
                            "could not fetch the reddit subreddit details"
                        );

                        send_reply(client, channel, plain_reply(subreddit_reply(&err)))?;
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
                warn!(
                    exception.message = %err,
                    "could not fetch the reddit submission details"
                );

                send_reply(client, channel, plain_reply(submission_reply(&err)))?;
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
        // The id is borrowed from the submission (or the fallback) — the mirror call only
        // reads it, so a copy would be a wasted allocation per mirrored submission.
        let id = submission.id.as_deref().unwrap_or(fallback_id);

        send_reply(client, channel, format_submission(&submission))?;

        // The media URL from the API is used for the download: reddit's anti-bot filters may
        // prevent `yt-dlp` from accessing the video through the link itself.
        if let Some(url) = submission.video_url()
            && let Some(mirror) = self.mirror.as_ref()
        {
            let Ok(media_id) = MediaId::parse(id) else {
                warn!(media.id = %id, "ignoring mirror request with an unsafe id");

                return Ok(());
            };

            plugins_shared::mirror::ensure_mirrored_if_configured(
                Some(mirror),
                url,
                &media_id,
                channel,
                client,
            )
            .await;
        }

        Ok(())
    }
}

/// Formats a failed submission lookup.
///
/// Only the stable domain reasons are appended; an internal error is reduced to the context
/// alone, because the caller logs it.
fn submission_reply(error: &RedditError) -> String {
    const CONTEXT: &str = "could not fetch the submission details";

    match error {
        RedditError::SubmissionNotFound | RedditError::InvalidId => format!("{CONTEXT}: {error}"),
        _ => CONTEXT.to_owned(),
    }
}

/// Formats a failed subreddit lookup.
fn subreddit_reply(error: &RedditError) -> String {
    const CONTEXT: &str = "could not fetch the subreddit details";

    match error {
        RedditError::SubredditNotFound | RedditError::InvalidSubreddit => {
            format!("{CONTEXT}: {error}")
        }
        _ => CONTEXT.to_owned(),
    }
}

/// Formats a failed video lookup.
fn video_reply(error: &RedditError) -> String {
    const CONTEXT: &str = "could not resolve the video link";

    match error {
        RedditError::InvalidId
        | RedditError::VideoRedirect
        | RedditError::VideoRedirectsToNonSubmission => format!("{CONTEXT}: {error}"),
        _ => CONTEXT.to_owned(),
    }
}

/// Formats a failed short-link resolution.
fn short_link_reply(error: &RedditError) -> String {
    const CONTEXT: &str = "could not resolve the shortened link";

    match error {
        RedditError::RedirectRedditLink
        | RedditError::InvalidId
        | RedditError::InvalidSubreddit => format!("{CONTEXT}: {error}"),
        _ => CONTEXT.to_owned(),
    }
}

/// Formats a submission's title and subreddit as a complete plain reply, unstyled for a single
/// IRC line. Callers send the result as-is.
fn format_submission(submission: &Submission) -> String {
    plain_reply(format_args!(
        "{} : {}",
        irc::unstyle_text(&submission.title),
        irc::unstyle_text(&submission.subreddit)
    ))
}

/// Formats a subreddit's title and public description as a labeled reply, unstyled for a single
/// IRC line.
///
/// The description is unstyled before it is truncated, so the character budget counts what is
/// actually sent.
fn format_subreddit(subreddit: &Subreddit) -> String {
    let title = irc::unstyle_text(&subreddit.title);
    let description = irc::unstyle_text(&subreddit.public_description);
    let description = description.truncate_with_suffix(250, "…");

    labeled_reply(&title, description)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;

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
    fn formats_submission_as_a_plain_reply() {
        let submission = submission("Sparindex S&amp;P500 børsnoteres i morgen", "dkfinance");

        assert_eq!(
            irc::strip_formatting(&format_submission(&submission)),
            "> Sparindex S&P500 børsnoteres i morgen : dkfinance"
        );
    }

    #[test]
    fn formats_subreddit_as_a_reply() {
        let subreddit = subreddit("r/Denmark &amp; friends", "News &amp; discussion");

        assert_eq!(
            irc::strip_formatting(&format_subreddit(&subreddit)),
            "> r/Denmark & friends: News & discussion"
        );
    }

    #[test]
    fn removes_control_characters_from_replies() {
        let submission = submission("Fake\x0310\nTitle", "dkfinance");
        let subreddit = subreddit("r/Denmark", "News&#10;and\x02 discussion");

        assert_eq!(
            irc::strip_formatting(&format_submission(&submission)),
            "> Fake Title : dkfinance"
        );
        assert_eq!(
            irc::strip_formatting(&format_subreddit(&subreddit)),
            "> r/Denmark: News and discussion"
        );
    }

    #[test]
    fn replies_name_the_safe_domain_reasons() {
        assert_eq!(
            submission_reply(&RedditError::SubmissionNotFound),
            "could not fetch the submission details: submission not found"
        );
        assert_eq!(
            submission_reply(&RedditError::InvalidId),
            "could not fetch the submission details: invalid reddit id"
        );
        assert_eq!(
            subreddit_reply(&RedditError::SubredditNotFound),
            "could not fetch the subreddit details: subreddit not found"
        );
        assert_eq!(
            subreddit_reply(&RedditError::InvalidSubreddit),
            "could not fetch the subreddit details: invalid subreddit name"
        );
        assert_eq!(
            video_reply(&RedditError::InvalidId),
            "could not resolve the video link: invalid reddit id"
        );
        assert_eq!(
            video_reply(&RedditError::VideoRedirect),
            "could not resolve the video link: video did not redirect to a reddit submission"
        );
        assert_eq!(
            video_reply(&RedditError::VideoRedirectsToNonSubmission),
            "could not resolve the video link: the video link redirects to something other than a \
             submission"
        );
        assert_eq!(
            short_link_reply(&RedditError::RedirectRedditLink),
            "could not resolve the shortened link: expected the short link to redirect to a \
             submission or comment"
        );
        assert_eq!(
            short_link_reply(&RedditError::InvalidSubreddit),
            "could not resolve the shortened link: invalid subreddit name"
        );
    }

    #[test]
    fn replies_hide_internal_errors() {
        assert_eq!(
            submission_reply(&RedditError::InvalidResponse),
            "could not fetch the submission details"
        );
        assert_eq!(
            subreddit_reply(&RedditError::InvalidResponse),
            "could not fetch the subreddit details"
        );
        assert_eq!(
            video_reply(&RedditError::InvalidResponse),
            "could not resolve the video link"
        );
        assert_eq!(
            short_link_reply(&RedditError::InvalidResponse),
            "could not resolve the shortened link"
        );
        assert!(!submission_reply(&RedditError::InvalidResponse).contains("deserialize"));
    }

    #[test]
    fn replies_only_surface_the_reasons_safe_for_the_operation() {
        assert_eq!(
            submission_reply(&RedditError::SubredditNotFound),
            "could not fetch the submission details"
        );
        assert_eq!(
            video_reply(&RedditError::SubmissionNotFound),
            "could not resolve the video link"
        );
        assert_eq!(
            short_link_reply(&RedditError::SubmissionNotFound),
            "could not resolve the shortened link"
        );
    }

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.client_id.is_none());
            assert!(settings.client_secret.is_none());
            assert!(settings.mirror.prefix.is_none());
            assert!(settings.mirror.public_url_base.is_none());
        }
        deserialize: {
            "client_id": "id",
            "client_secret": "secret",
            "prefix": "~meta/reddit",
            "public_url_base": "https://pub.example.com/reddit",
        } assert: {
            assert_eq!(settings.client_id.as_deref(), Some("id"));
            assert_eq!(settings.client_secret.as_deref(), Some("secret"));
            assert_eq!(settings.mirror.prefix.as_deref(), Some("~meta/reddit"));
            assert_eq!(
                settings.mirror.public_url_base.as_deref(),
                Some("https://pub.example.com/reddit")
            );
        }
    }
}
