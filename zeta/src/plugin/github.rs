//! Searches GitHub repositories and reports the most starred match.
//!
//! The `.gh <query>` command searches the GitHub repository search API (sorted by stars) and
//! replies with the top repository: full name, description, URL, language, and star count,
//! prefixed with a fork mark for forks. An empty query replies with usage; API errors and
//! empty results are reported in the reply.
//!
//! A GitHub API token — set in `[plugins.github]`, falling back to the `GITHUB_TOKEN`
//! environment variable — raises the API rate limit. The token is optional: a missing one
//! does not prevent the plugin from initializing.

use std::fmt::Write;

use reqwest::{
    self,
    header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::{config::HttpConfig, error::RequestError, http, plugin::prelude::*};

/// Settings for the github plugin, from its `[plugins.github]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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
    /// Sending the HTTP request failed.
    #[error("request error: {0}")]
    Request(#[from] RequestError),
    /// The configured token is not a valid header value.
    #[error("invalid header value: {0}")]
    InvalidToken(#[from] reqwest::header::InvalidHeaderValue),
    /// The GitHub API returned an error response.
    #[error(transparent)]
    Api(#[from] http::ApiError),
}

/// The `.gh` command.
const GITHUB: CommandSpec = CommandSpec::new(
    ".gh",
    "Search GitHub and show the most starred match",
);

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

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.command(GITHUB);
        // The token is optional; it only raises the API rate limit.
        let token = resolve_secret(settings.token.as_deref(), "GITHUB_TOKEN").ok();
        let plugin = GitHubPlugin::new(&ctx.config.http, token).map_err(plugin_err)?;

        Ok(plugin)
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let query = command.args().trim();

        let response = if query.is_empty() {
            reply("GitHub", ".gh <query>")
        } else {
            info!(query, %channel, "searching github");

            match self.search_repos(query).await {
                Ok(response) => response.items.first().map_or_else(
                    || reply("GitHub", "No results"),
                    Self::format_repo_details,
                ),
                Err(err) => reply("GitHub", err),
            }
        };

        client.send_privmsg(channel, response)?;

        Ok(())
    }
}

impl GitHubPlugin {
    /// Creates a new instance of the GitHub plugin.
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

        let client = http::builder(config)
            .default_headers(headers)
            .build()
            .map_err(|error| Error::Request(RequestError::from(error)))?;

        Ok(Self { http: client })
    }

    /// Searches for repositories matching `query`.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the GitHub search request fails or its response cannot be
    /// parsed.
    async fn search_repos(&self, query: &str) -> Result<SearchResponse, Error> {
        let params = [("q", query), ("sort", "stars"), ("order", "desc")];

        let request = self
            .http
            .get("https://api.github.com/search/repositories")
            .query(&params);
        http::get_json(request).await.map_err(Error::from)
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

        reply("GitHub", &line)
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
