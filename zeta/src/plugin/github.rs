use std::fmt::Write;

use reqwest::{
    self,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::{config::HttpConfig, http, plugin::prelude::*};

/// Settings for the github plugin, from its `[plugins.github]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// A GitHub API token, used to raise the rate limit for requests.
    ///
    /// Falls back to the `GITHUB_TOKEN` environment variable when unset.
    #[serde(default)]
    pub token: Option<String>,
}

/// Errors that can occur during GitHub interaction.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("invalid header value: {0}")]
    InvalidToken(#[from] reqwest::header::InvalidHeaderValue),
    #[error(transparent)]
    Api(#[from] http::ApiError),
}

/// The `.gh` command.
const GITHUB: PluginCommand = PluginCommand::new(
    Prefix::new(".gh"),
    "Search GitHub and show the most starred match",
);

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[GITHUB];

/// Structure representing the GitHub Plugin.
/// Holds the HTTP client to reuse connection pools.
pub struct GitHubPlugin {
    http: reqwest::Client,
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
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        // The token is optional; it only raises the API rate limit.
        let token = resolve_secret(settings.token.as_deref(), "GITHUB_TOKEN").ok();
        let plugin = GitHubPlugin::new(&ctx.config.http, token).map_err(plugin_err)?;

        Ok(plugin)
    }

    const COMMANDS: &'static [PluginCommand] = COMMANDS;

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        channel: &str,
        _command: &Prefix,
        args: &str,
    ) -> Result<(), ZetaError> {
        if let Ok(Some(response)) = self.run(channel, Some(args)).await {
            client.send_privmsg(channel, response)?;
        } else {
            client.send_privmsg(channel, "no results")?;
        }

        Ok(())
    }
}

impl GitHubPlugin {
    /// Create a new instance of the GitHub plugin.
    /// Initializes a generic HTTP client with standard timeouts.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP client could not be built, or the token is not a valid
    /// header value.
    pub fn new(config: &HttpConfig, token: Option<String>) -> Result<Self, Error> {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );

        if let Some(token) = token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {token}"))?,
            );
        }

        let client = http::client::builder(config)
            .default_headers(headers)
            .build()?;

        Ok(Self { http: client })
    }

    /// The main entry point for processing the `.gh` command.
    ///
    /// # Arguments
    /// * `channel` - The target channel (used for logging or context).
    /// * `args` - The command arguments (the query).
    ///
    /// # Returns
    /// * `Result<Option<String>>` - Some(message) to reply, or None if no reply needed.
    pub async fn run(&self, channel: &str, args: Option<&str>) -> Result<Option<String>, Error> {
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
    async fn search_repos(&self, query: &str) -> Result<SearchResponse, Error> {
        let params = [("q", query), ("sort", "stars"), ("order", "desc")];

        let response = self
            .http
            .get("https://api.github.com/search/repositories")
            .query(&params)
            .send()
            .await?;

        http::parse_response(response).await.map_err(Error::from)
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

    /// Formats the final message with the standard prefix.
    fn format_message(message: &str) -> String {
        reply("GitHub", message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.token.is_none());
        }
        deserialize: {
            "token": "secret",
        } assert: {
            assert_eq!(settings.token.as_deref(), Some("secret"));
        }
    }

    #[test]
    fn invalid_tokens_are_rejected() {
        assert!(
            GitHubPlugin::new(&HttpConfig::default(), Some("bad\ntoken".to_string())).is_err(),
            "invalid token should be rejected"
        );
    }
}
