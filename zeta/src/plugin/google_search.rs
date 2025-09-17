use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use scraper::{Html, Selector};

use crate::Error as ZetaError;
use crate::command::Command as ZetaCommand;
use crate::plugin;

use super::{Author, Name, Plugin, Version};

/// Represents a single search result obtained from the search operation.
pub struct SearchResult {
    /// The title of the search result.
    pub title: String,
    /// The URL of the search result.
    pub url: String,
    /// A brief snippet or description from the search result.
    #[allow(dead_code)]
    pub snippet: String,
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("server returned invalid response")]
    InvalidResponse,
    #[error("unable to read contents")]
    ReadContents,
    #[error("missing expected HTML element: {0}")]
    MissingElement(String),
}

pub struct GoogleSearch {
    client: reqwest::Client,
    command: ZetaCommand,
    article_selector: Selector,
    a_selector: Selector,
    p_selector: Selector,
    h3_selector: Selector,
}

#[async_trait]
impl Plugin for GoogleSearch {
    /// Creates a new instance of the [`GoogleSearch`] plugin.
    ///
    /// Initializes an HTTP client with a specific user agent, no redirects, and a timeout.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be built.
    fn new() -> GoogleSearch {
        let client = plugin::build_http_client();

        GoogleSearch::with_client(client)
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
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(query) = self.command.parse(user_message)
        {
            let results = self
                .search(query.trim())
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

        Ok(())
    }
}

impl GoogleSearch {
    pub fn with_client(client: reqwest::Client) -> Self {
        let command = ZetaCommand::new(".g");

        Self {
            client,
            command,
            article_selector: Selector::parse("main article").unwrap(),
            a_selector: Selector::parse("a[href]").unwrap(),
            p_selector: Selector::parse("p").unwrap(),
            h3_selector: Selector::parse("h3").unwrap(),
        }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>, Error> {
        let params = [("q", query), ("engine", "google")];
        let request = self
            .client
            .get("https://leta.mullvad.net/search")
            .query(&params);
        let response = request.send().await.map_err(|_| Error::InvalidResponse)?;
        let html_content = response.text().await.map_err(|_| Error::ReadContents)?;
        let document = Html::parse_document(&html_content);
        let mut results = Vec::new();

        // Iterate over each search result article in the parsed document
        for article in document.select(&self.article_selector) {
            let link = article.select(&self.a_selector).next();
            let title = link.and_then(|x| x.select(&self.h3_selector).next());
            let snippet = article.select(&self.p_selector).next();

            if let (Some(title), Some(link), Some(snippet)) = (title, link, snippet) {
                let url = link
                    .attr("href")
                    .ok_or_else(|| {
                        Error::MissingElement(
                            "href attribute missing from link element".to_string(),
                        )
                    })?
                    .to_string();
                let title_text: String = title.text().map(str::trim).collect();
                let snippet_text: String = snippet.text().map(str::trim).collect();

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
