use reqwest::header::ToStrError;
use tracing::error;
use url::Url;

use crate::{
    http,
    plugin::{self, prelude::*},
    utils::Truncatable,
};

mod api;
mod types;

pub use types::{Item, Link, Submission, Subreddit};

pub const REDDIT_BASE_URL: &str = "https://www.reddit.com";

pub struct Reddit {
    client: reqwest::Client,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("request error: {0}")]
    Reqwest(#[source] reqwest::Error),
    #[error("could not deserialize comments json: {0}")]
    DeserializeComments(#[source] serde_path_to_error::Error<serde_json::Error>),
    #[error("could not deserialize subreddit json: {0}")]
    DeserializeSubreddit(#[source] serde_path_to_error::Error<serde_json::Error>),
    #[error("subreddit was not found")]
    SubredditNotFound,
    #[error("submission not found")]
    SubmissionNotFound,
    #[error("http error: {0}")]
    Http(#[source] reqwest::Error),
    #[error("could not deserialize response as it is in unexpected format")]
    InvalidResponse,
    #[error("the shortened link did not return a usable redirect url")]
    InvalidRedirect,
    #[error("the response redirect url is using an invalid encoding: {0}")]
    RedirectUrlEncoding(ToStrError),
    #[error("expected the short link to redirect to a submission or comment")]
    RedirectRedditLink,
}

#[async_trait]
impl Plugin<Context> for Reddit {
    fn new(_ctx: &Context) -> Result<Self, ZetaError> {
        Ok(Reddit {
            client: http::build_client(),
        })
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
            if let Some(link) = Reddit::parse_reddit_url(url) {
                self.process_url(link, channel, client).await?;
            }
        }

        Ok(())
    }

    async fn process_url(&self, link: Link, channel: &str, client: &Client) -> Result<(), Error> {
        match link {
            Link::Gallery(id) | Link::Comments { id } | Link::Submission { id, .. } => {
                match self.submission(&id).await {
                    Ok(submission) => {
                        let title = submission.title;
                        let subreddit = submission.subreddit;

                        if let Err(e) = client
                            .send_privmsg(channel, format!("\x0310> {title} : {subreddit}"))
                    {
                        error!("failed to send message: {e}");
                    }
                    }
                    Err(err) => {
                        if let Err(e) = client
                            .send_privmsg(
                                channel,
                                format!("\x0310> could not fetch submission details: {err}"),
                            )
                    {
                        error!("failed to send message: {e}");
                    }
                    }
                }
            }
            Link::Comment { submission, .. } => match self.submission(&submission).await {
                Ok(submission) => {
                    let title = submission.title;
                    let subreddit = submission.subreddit;

                    if let Err(e) = client
                        .send_privmsg(channel, format!("\x0310> {title} : {subreddit}"))
                {
                    error!("failed to send message: {e}");
                }
                }
                Err(err) => {
                    if let Err(e) = client
                        .send_privmsg(
                            channel,
                            format!("\x0310> could not fetch submission details: {err}"),
                        )
                {
                    error!("failed to send message: {e}");
                }
                }
            },
            Link::Shortened { id, subreddit } => {
                match self.resolve_shortened_link(&subreddit, &id).await {
                    Ok(link) => {
                        if let Err(e) = Box::pin(self.process_url(link, channel, client)).await {
                            error!("failed to process resolved link: {e}");
                        }
                    }
                    Err(err) => {
                        if let Err(e) = client
                            .send_privmsg(
                                channel,
                                format!("\x0310> could not resolve shortened link: {err}"),
                            )
                    {
                        error!("failed to send message: {e}");
                    }
                    }
                }
            }
            Link::Subreddit(subreddit) => match self.subreddit_about_info(&subreddit).await {
                Ok(subreddit) => {
                    let title = subreddit.title;
                    let description = subreddit.public_description.truncate_with_suffix(250, "…");

                    if let Err(e) = client
                        .send_privmsg(
                            channel,
                            format!("\x0310>\x03\x02 {title}:\x02\x0310 {description}"),
                        )
                    {
                        error!("failed to send message: {e}");
                    }
                }
                Err(err) => {
                    if let Err(e) = client
                        .send_privmsg(
                            channel,
                            format!("\x0310> could not fetch subreddit details: {err}"),
                        )
                {
                    error!("failed to send message: {e}");
                }
                }
            },
            _ => {}
        }

        Ok(())
    }
}
