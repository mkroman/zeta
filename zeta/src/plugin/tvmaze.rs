//! Looks up TV shows in TVmaze and reports their next episode.
//!
//! The `.next <show>` command searches TVmaze for the show (single search, first match) and
//! replies with when its next episode airs — title, season and episode numbers, and the time
//! until the airstamp in words — or, when no next episode is scheduled, with the show's
//! current status ("Running", "Ended"). An unknown show, a failed request, and other errors
//! are reported in the channel.
//!
//! The TVmaze API is public and needs no credentials. The plugin has no settings.
use reqwest::Url;
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::{
    config::HttpConfig,
    duration::TimeInWords,
    error::RequestError,
    http,
    plugin::prelude::*,
    url::redact_url,
};

/// Base URL for the TVmaze API.
pub const API_BASE_URL: &str = "https://api.tvmaze.com";

/// Errors that can occur while talking to the TVmaze API.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The API request failed or its response could not be handled.
    #[error(transparent)]
    Api(#[from] http::ApiError),
    /// No show matches the search query.
    #[error("resource not found")]
    NotFound,
}

impl From<RequestError> for Error {
    fn from(error: RequestError) -> Self {
        Self::Api(error.into())
    }
}

/// The `.next` command.
const NEXT: CommandSpec = CommandSpec::new(".next", "Show when a show's next episode airs");

/// The TVmaze plugin: looks up shows and their next episodes.
pub struct Tvmaze {
    /// HTTP client for API requests.
    client: reqwest::Client,
    /// Cached endpoint URLs for performance.
    urls: EndpointUrls,
}

/// Represents a TV show from the TVmaze API.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Show {
    /// Unique TVmaze identifier for the show.
    id: u64,
    /// URL to the show's details page.
    url: String,
    /// Name of the show.
    name: String,
    /// Type of show (e.g., "Scripted")
    #[serde(rename = "type")]
    #[allow(clippy::struct_field_names)]
    show_type: Option<String>,
    /// Primary language of the show.
    language: Option<String>,
    /// List of genres associated with the show.
    genres: Vec<String>,
    /// Current status of the show (e.g., "Running", "Ended").
    status: String,
    /// Runtime of episodes in minutes.
    runtime: Option<u64>,
    /// Average runtime of episodes in minutes.
    average_runtime: Option<u64>,
    /// Date when the show premiered.
    premiered: Option<String>,
    /// Date when the show ended.
    ended: Option<String>,
    /// URL to an official website.
    official_site: Option<String>,
    /// External service identifiers.
    externals: Option<Externals>,
    /// Embedded response data.
    #[serde(rename = "_embedded")]
    embedded: Option<Embedded>,
}

/// External service identifiers for a show.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Externals {
    /// TVRage resource ID, if available.
    tvrage: Option<u64>,
    /// TheTVDB resource ID, if available.
    thetvdb: Option<u64>,
    /// IMDb resource ID, if available.
    imdb: Option<String>,
}

/// Embedded data in API responses.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Embedded {
    /// Next episode information, if available.
    #[serde(rename = "nextepisode")]
    next_episode: Option<Episode>,
    /// List of episodes, if available.
    episodes: Option<Vec<Episode>>,
}

/// Represents an episode from the TVmaze API.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Episode {
    /// Unique TVmaze identifier for the episode.
    pub id: u64,
    /// Name of the episode.
    pub name: String,
    /// Season number.
    pub season: u64,
    /// Episode number within the season.
    pub number: u64,
    /// Timestamp when the episode airs.
    #[serde(with = "time::serde::rfc3339::option")]
    pub airstamp: Option<time::OffsetDateTime>,
}

/// Cached collection of API endpoint URLs.
pub struct EndpointUrls {
    /// URL for single show search endpoint.
    pub single_search: Url,
}

impl Default for EndpointUrls {
    fn default() -> Self {
        Self::new()
    }
}

impl EndpointUrls {
    /// Builds the collection of TVmaze endpoint URLs.
    ///
    /// # Panics
    ///
    /// Panics if the API base URL fails to parse, which can only happen on a programming error
    /// since the base URL is a constant.
    #[must_use]
    pub fn new() -> EndpointUrls {
        EndpointUrls {
            single_search: Url::parse(&format!("{API_BASE_URL}/singlesearch/shows"))
                .expect("single search url"),
        }
    }
}

#[async_trait]
impl Plugin<Context> for Tvmaze {
    type Settings = NoSettings;

    fn new(ctx: &Context, _settings: &NoSettings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.command(NEXT);
        Ok(Tvmaze::new(&ctx.config.http))
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        self.handle_show_search(command.args(), command.channel(), client).await
    }
}

impl Tvmaze {
    /// Creates a new TVmaze plugin instance.
    #[must_use]
    pub fn new(config: &HttpConfig) -> Self {
        let client = http::build_client(config);
        let urls = EndpointUrls::new();

        Tvmaze { client, urls }
    }

    /// Searches for a single show using the TVmaze API.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the show to search for
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotFound`] if no show matches the search query, and an
    /// [`Error::Api`] if the request fails or the response cannot be handled.
    #[instrument(skip(self))]
    pub async fn single_search(&self, name: &str) -> Result<Show, Error> {
        let url = self.build_search_url(name);
        debug!(
            url.full = %redact_url(&url),
            "requesting single search for show: {name}"
        );

        let response = http::send(self.client.get(url)).await?;
        let show = http::parse_response_or_404(response, Error::NotFound).await?;

        debug!(?show, "finished parsing show");

        Ok(show)
    }

    /// Handles the search response and parses the show data.
    async fn handle_show_search(
        &self,
        name: &str,
        channel: &str,
        client: &Client,
    ) -> Result<(), ZetaError> {
        match self.single_search(name).await {
            Ok(show) => {
                let message = Self::format_show_message(&show);

                client.send_privmsg(channel, message)?;
            }
            Err(err) => {
                let error_message = Self::format_error_message(&err);

                client.send_privmsg(channel, &error_message)?;
            }
        }

        Ok(())
    }

    /// Formats a show message based on whether there's a next episode.
    fn format_show_message(show: &Show) -> String {
        show.embedded
            .as_ref()
            .and_then(|e| e.next_episode.as_ref())
            .map_or_else(
                || Self::format_show_status_message(show),
                |episode| Self::format_next_episode_message(show, episode),
            )
    }

    /// Formats a message about the next episode.
    fn format_next_episode_message(show: &Show, episode: &Episode) -> String {
        let title = &episode.name;
        let season = episode.season;
        let number = episode.number;
        let time_until_air = {
            let now = time::OffsetDateTime::now_utc();
            episode
                .airstamp
                .map_or_else(|| "???".to_string(), |airstamp| (airstamp - now).in_words())
        };
        let content = format!(
            "Next episode “\x0f{title}\x0310” (\x0f{season}x{number:02}\x0310) airs in\x0f {time_until_air}"
        );

        Self::build_formatted_message(Some(&show.name), &content)
    }

    /// Formats a message about the show's current status.
    fn format_show_status_message(show: &Show) -> String {
        let name = &show.name;
        let status = &show.status;
        let content = format!(
            "\x0f{name}\x0310 is currently marked as\x0f {status}\x0310 and there is no next episode"
        );

        Self::build_formatted_message(None, &content)
    }

    /// Formats an error message for display.
    fn format_error_message(error: &Error) -> String {
        let content = match error {
            Error::NotFound => "Show not found".to_string(),
            Error::Api(_) => "Failed to fetch show information".to_string(),
        };

        Self::build_formatted_message(None, &content)
    }

    /// Builds the search URL with query parameters.
    fn build_search_url(&self, query: &str) -> Url {
        let mut url = self.urls.single_search.clone();

        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("embed", "nextepisode");
        url
    }

    /// Formats a message for display in IRC with an optional bold subject name.
    fn build_formatted_message(prefix: Option<&str>, message: &str) -> String {
        prefix.map_or_else(
            || reply("TVmaze", message),
            |name| reply("TVmaze", format!("(\x0f{name}\x0310): {message}")),
        )
    }
}
