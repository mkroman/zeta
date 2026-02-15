use std::fmt::Write;

use miette::{Diagnostic, Result};
use reqwest::{
    self,
    header::{ACCEPT, HeaderMap, HeaderValue},
};
use serde::Deserialize;
use thiserror::Error;
use tracing::{error, info};

use crate::{http, plugin::prelude::*};

/// Custom error types for the GitHub plugin.
#[derive(Debug, Error, Diagnostic)]
pub enum Error {
    #[error("Failed to initialize GitHub HTTP client")]
    #[diagnostic(code(github::init))]
    InitFailed(#[source] reqwest::Error),

    #[error("Failed to perform GitHub search request")]
    #[diagnostic(code(github::search::request))]
    SearchRequestFailed(#[source] reqwest::Error),

    #[error("GitHub API returned status {0}")]
    #[diagnostic(code(github::search::status))]
    ApiStatus(reqwest::StatusCode),

    #[error("Failed to parse GitHub response")]
    #[diagnostic(code(github::search::parse))]
    ResponseParseFailed(#[source] reqwest::Error),
}

/// Structure representing the GitHub Plugin.
/// Holds the HTTP client to reuse connection pools.
pub struct GitHubPlugin {
    http: reqwest::Client,
    command: ZetaCommand,
}

/// Represents the top-level search response from GitHub API.
#[derive(Debug, Deserialize)]
struct SearchResponse {
    items: Vec<RepoItem>,
}

/// Represents a single repository item from the GitHub API.
#[derive(Debug, Deserialize)]
struct RepoItem {
    full_name: String,
    description: Option<String>,
    html_url: String,
    language: Option<String>,
    stargazers_count: u64,
    fork: bool,
}

#[async_trait]
impl Plugin<Context> for GitHubPlugin {
    fn new(_ctx: &Context) -> Self {
        GitHubPlugin::new().expect("could not create github plugin")
    }

    fn name() -> Name {
        Name::from("github")
    }

    fn author() -> Author {
        Author::from("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version::from("0.1")
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(args) = self.command.parse(user_message)
        {
            if let Ok(Some(response)) = self.handle_command(channel, Some(args)).await {
                client.send_privmsg(channel, response)?;
            } else {
                client.send_privmsg(channel, "no results")?;
            }
        }
        Ok(())
    }
}

impl GitHubPlugin {
    /// Create a new instance of the GitHub plugin.
    /// Initializes a generic HTTP client with standard timeouts.
    pub fn new() -> Result<Self> {
        let cmd = ZetaCommand::new(".gh");
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );

        let client = http::client::builder()
            .default_headers(headers)
            .build()
            .map_err(Error::InitFailed)?;

        Ok(Self {
            http: client,
            command: cmd,
        })
    }

    /// The main entry point for processing the `.gh` command.
    ///
    /// # Arguments
    /// * `channel` - The target channel (used for logging or context).
    /// * `args` - The command arguments (the query).
    ///
    /// # Returns
    /// * `Result<Option<String>>` - Some(message) to reply, or None if no reply needed.
    pub async fn handle_command(
        &self,
        channel: &str,
        args: Option<&str>,
    ) -> Result<Option<String>> {
        // 1. Check arguments
        let query = match args {
            Some(q) if !q.trim().is_empty() => q.trim(),
            _ => return Ok(Some(Self::usage_information())),
        };

        info!("Searching GitHub for '{}' in channel {}", query, channel);

        // 2. Perform Search
        match self.search_repos(query).await {
            Ok(response) => {
                // 3. Process Result
                response.items.first().map_or_else(
                    || Ok(Some(Self::format_message("No results"))),
                    |first_result| Ok(Some(Self::format_repo_details(first_result))),
                )
            }
            Err(e) => {
                error!("GitHub API error: {:?}", e);
                // In a real bot, you might want to sanitize this error message
                Ok(Some(Self::format_message(&format!("http error: {e}"))))
            }
        }
    }

    /// Searches for repositories based on the given query.
    async fn search_repos(&self, query: &str) -> Result<SearchResponse> {
        let params = [("q", query), ("sort", "stars"), ("order", "desc")];

        let response = self
            .http
            .get("https://api.github.com/search/repositories")
            .query(&params)
            .send()
            .await
            .map_err(Error::SearchRequestFailed)?;

        // Check for HTTP errors (4xx, 5xx)
        if !response.status().is_success() {
            return Err(Error::ApiStatus(response.status()).into());
        }

        let search_data: SearchResponse =
            response.json().await.map_err(Error::ResponseParseFailed)?;

        Ok(search_data)
    }

    /// Formats a specific repository item into an IRC-friendly string.
    fn format_repo_details(item: &RepoItem) -> String {
        let mut line = String::new();

        // Fork icon: "\x0f\u2442\x0310 "
        if item.fork {
            let _ = write!(line, "\x0F\u{2442}\x0310 ");
        }

        line.push_str(&item.full_name);

        if let Some(desc) = &item.description {
            let _ = write!(line, " - {desc}");
        }

        // " -\x0f <url>"
        let _ = write!(line, " -\x0F {}", item.html_url);

        // "\x0310 - Language:\x0f <lang>\x0310 Stars:\x0f <stars>"
        let lang = item.language.as_deref().unwrap_or("?");
        let _ = write!(
            line,
            "\x0310 - Language:\x0F {}\x0310 Stars:\x0F {}",
            lang, item.stargazers_count
        );

        Self::format_message(&line)
    }

    /// Usage information helper.
    fn usage_information() -> String {
        Self::format_message(".gh <query>")
    }

    /// Formats the final message with the standard Zeta/Blur prefix.
    /// Ruby: %(\x0310>\x0F\x02 GitHub:\x02\x0310 #{message})
    fn format_message(message: &str) -> String {
        format!("\x0310>\x0F\x02 GitHub:\x02\x0310 {message}")
    }
}
