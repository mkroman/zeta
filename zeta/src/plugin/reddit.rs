use reddit::Link;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use tracing::error;
use url::Url;

use crate::{
    plugin::{self, prelude::*},
    utils::Truncatable,
};

/// Identifying HTTP user agent for API requests (i.e. `linux:zeta:<VERSION> (by /u/drizz)`)
pub const USER_AGENT: &str = concat!("linux:zeta:", env!("CARGO_PKG_VERSION"), " (by /u/drizz)");

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

        Ok(Reddit { client })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "reddit".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(urls) = plugin::extract_urls(user_message)
        {
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
                match self.client.submission(&id).await {
                    Ok(submission) => {
                        let title = submission.title;
                        let subreddit = submission.subreddit;

                        client.send_privmsg(channel, format!("\x0310> {title} : {subreddit}"))?;
                    }
                    Err(err) => {
                        client.send_privmsg(
                            channel,
                            format!("\x0310> could not fetch submission details: {err}"),
                        )?;
                    }
                }
            }
            Link::Comment { submission, .. } => match self.client.submission(&submission).await {
                Ok(submission) => {
                    let title = submission.title;
                    let subreddit = submission.subreddit;

                    client.send_privmsg(channel, format!("\x0310> {title} : {subreddit}"))?;
                }
                Err(err) => client.send_privmsg(
                    channel,
                    format!("\x0310> could not fetch submission details: {err}"),
                )?,
            },
            Link::Video(id) => match self.client.video(&id).await {
                Ok(submission) => {
                    let title = submission.title;
                    let subreddit = submission.subreddit;

                    client.send_privmsg(channel, format!("\x0310> {title} : {subreddit}"))?;
                }
                Err(err) => {
                    client.send_privmsg(
                        channel,
                        format!("\x0310> could not resolve video link: {err}"),
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
                            format!("\x0310> could not resolve shortened link: {err}"),
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

                        client.send_privmsg(
                            channel,
                            format!("\x0310>\x03\x02 {title}:\x02\x0310 {description}"),
                        )?;
                    }
                    Err(err) => {
                        client.send_privmsg(
                            channel,
                            format!("\x0310> could not fetch subreddit details: {err}"),
                        )?;
                    }
                }
            }
            _ => {}
        }

        Ok(())
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
    }

    #[test]
    fn settings_deserialize() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "client_id": "id",
            "client_secret": "secret",
        }))
        .expect("could not deserialize settings");

        assert_eq!(settings.client_id.as_deref(), Some("id"));
        assert_eq!(settings.client_secret.as_deref(), Some("secret"));
    }
}
