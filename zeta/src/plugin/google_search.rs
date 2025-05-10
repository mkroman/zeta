use std::time::Duration;

use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use reqwest::redirect::Policy;
use scraper::{Html, Selector};

use crate::consts;
use crate::Error as ZetaError;

use super::{Author, Name, Plugin, Version};

pub struct GoogleSearch {
    http_client: reqwest::Client,
}

#[async_trait]
impl Plugin for GoogleSearch {
    fn new() -> GoogleSearch {
        let http_client = reqwest::ClientBuilder::new()
            .user_agent(consts::USER_AGENT)
            .redirect(Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .expect("could not build http client");

        GoogleSearch { http_client }
    }

    fn name() -> Name {
        Name("google_search")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref inner_message) = message.command {
            if let Some(query) = inner_message.strip_prefix(".g ") {
                let results = self
                    .search(query)
                    .await
                    .map_err(|err| ZetaError::PluginError(Box::new(err)))?;

                if let Some(result) = results.first() {
                    client
                        .send_privmsg(
                            channel,
                            format!("\x0310> {} - {}", result.title, result.url),
                        )
                        .map_err(ZetaError::IrcClientError)?;
                } else {
                    client
                        .send_privmsg(channel, "\x0310> No results")
                        .map_err(ZetaError::IrcClientError)?;
                }
            }
        }

        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Debug)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("server returned invalid response")]
    InvalidResponse,
    #[error("unable to read contents")]
    ReadContents,
}

impl GoogleSearch {
    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>, Error> {
        let params = [("q", query), ("engine", "google")];
        let request = self
            .http_client
            .get("https://leta.mullvad.net/search")
            .query(&params);
        let response = request.send().await.map_err(|_| Error::InvalidResponse)?;
        let html = response.text().await.map_err(|_| Error::ReadContents)?;
        let document = Html::parse_document(&html);
        let mut results = vec![];

        let article_selector = Selector::parse("main article").unwrap();
        let a_selector = Selector::parse("a[href]").unwrap();
        let p_selector = Selector::parse("p").unwrap();
        let h3_selector = Selector::parse("h3").unwrap();

        for article in document.select(&article_selector) {
            let link = article.select(&a_selector).next();
            let title = link.and_then(|x| x.select(&h3_selector).next());
            let snippet = article.select(&p_selector).next();

            if let (Some(title), Some(link), Some(snippet)) = (title, link, snippet) {
                let url = link
                    .attr("href")
                    .expect("title link does not have a href attribute")
                    .to_string();
                let title_text = title.text().map(str::trim).collect::<String>();
                let snippet_text = snippet.text().map(str::trim).collect::<String>();

                let result = SearchResult {
                    url,
                    snippet: snippet_text,
                    title: title_text,
                };
                results.push(result);
            }
        }

        Ok(results)
    }
}
