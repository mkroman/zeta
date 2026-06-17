use reddit::Link;
use secrecy::SecretString;
use tracing::error;
use url::Url;

use crate::{
    plugin::{self, prelude::*},
    utils::Truncatable,
};

/// Identifying HTTP user agent for API requests (i.e. `linux:zeta:<VERSION> (by /u/drizz)`)
pub const USER_AGENT: &str = concat!("linux:zeta:", env!("CARGO_PKG_VERSION"), " (by /u/drizz)");

/// Errors that can occur during Reddit interaction
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
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
    fn new(_ctx: &Context) -> Result<Self, ZetaError> {
        let client_id = require_env("REDDIT_CLIENT_ID")?;
        let client_secret: SecretString = require_env("REDDIT_CLIENT_SECRET")?.into();
        let user_agent = Some(USER_AGENT.to_string());
        let client = reddit::Client::new(client_id, client_secret, user_agent);

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
            if let Some(link) = reddit::parse_reddit_url(url) {
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

                        if let Err(e) =
                            client.send_privmsg(channel, format!("\x0310> {title} : {subreddit}"))
                        {
                            error!("failed to send message: {e}");
                        }
                    }
                    Err(err) => {
                        if let Err(e) = client.send_privmsg(
                            channel,
                            format!("\x0310> could not fetch submission details: {err}"),
                        ) {
                            error!("failed to send message: {e}");
                        }
                    }
                }
            }
            Link::Comment { submission, .. } => match self.client.submission(&submission).await {
                Ok(submission) => {
                    let title = submission.title;
                    let subreddit = submission.subreddit;

                    if let Err(e) =
                        client.send_privmsg(channel, format!("\x0310> {title} : {subreddit}"))
                    {
                        error!("failed to send message: {e}");
                    }
                }
                Err(err) => {
                    if let Err(e) = client.send_privmsg(
                        channel,
                        format!("\x0310> could not fetch submission details: {err}"),
                    ) {
                        error!("failed to send message: {e}");
                    }
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
                        if let Err(e) = client.send_privmsg(
                            channel,
                            format!("\x0310> could not resolve shortened link: {err}"),
                        ) {
                            error!("failed to send message: {e}");
                        }
                    }
                }
            }
            Link::Subreddit(subreddit) => {
                match self.client.subreddit_about_info(&subreddit).await {
                    Ok(subreddit) => {
                        let title = subreddit.title;
                        let description =
                            subreddit.public_description.truncate_with_suffix(250, "…");

                        if let Err(e) = client.send_privmsg(
                            channel,
                            format!("\x0310>\x03\x02 {title}:\x02\x0310 {description}"),
                        ) {
                            error!("failed to send message: {e}");
                        }
                    }
                    Err(err) => {
                        if let Err(e) = client.send_privmsg(
                            channel,
                            format!("\x0310> could not fetch subreddit details: {err}"),
                        ) {
                            error!("failed to send message: {e}");
                        }
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }
}
