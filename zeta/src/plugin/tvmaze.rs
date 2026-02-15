//! TVmaze API integration plugin.
//!
//! This plugin provides functionality to search for TV shows and display information
//! about upcoming episodes using the TVmaze API.
use reqwest::{Response, StatusCode, Url};
use serde::{Deserialize, de::DeserializeOwned};
use time::Duration;
use tracing::{debug, error, instrument};

use crate::{http, plugin::prelude::*};

/// Base URL for the TVmaze API.
pub const API_BASE_URL: &str = "https://api.tvmaze.com";

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("could not deserialize response: {0}")]
    Deserialize(#[source] serde_path_to_error::Error<serde_json::Error>),
    #[error("resource not found")]
    NotFound,
    #[error("request error: {0}")]
    Request(#[source] reqwest::Error),
    #[error("unexpected http response")]
    UnexpectedResponse,
}

pub struct Tvmaze {
    /// HTTP client for API requests.
    client: reqwest::Client,
    /// Command handler for the `.next` command.
    command: ZetaCommand,
    /// Cached endpoint URLs for performance.
    urls: EndpointUrls,
}

/// Represents a TV show from the TVmaze API.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename = "camelCase")]
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

/// Cachced collection of API endpoint URLs.
pub struct EndpointUrls {
    /// URL for single show search endpoint.
    pub single_search: Url,
}

impl EndpointUrls {
    pub fn new() -> EndpointUrls {
        EndpointUrls {
            single_search: Url::parse(&format!("{API_BASE_URL}/singlesearch/shows"))
                .expect("single search url"),
        }
    }
}

#[async_trait]
impl Plugin<Context> for Tvmaze {
    fn new(_ctx: &Context) -> Self {
        Tvmaze::new()
    }

    fn name() -> Name {
        Name::from("tvmaze")
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
            self.handle_show_search(args, channel, client).await?;
        }

        Ok(())
    }
}

impl Tvmaze {
    /// Creates a new TVmaze plugin instance.
    pub fn new() -> Self {
        let client = http::build_client();
        let command = ZetaCommand::new(".next");
        let urls = EndpointUrls::new();

        Tvmaze {
            client,
            command,
            urls,
        }
    }

    /// Searches for a single show using the TVmaze API.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the show to search for
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if no show matches the search query.
    /// Returns `Error::Request` if the HTTP request fails.
    /// Returns `Error::Deserialize` if the response cannot be parsed.
    #[instrument(skip(self))]
    pub async fn single_search(&self, name: &str) -> Result<Show, Error> {
        let url = self.build_search_url(name);
        debug!(url.full = %url, "requesting single search for show: {name}");

        let response = self.client.get(url).send().await.map_err(Error::Request)?;

        Self::handle_search_response(response).await
    }

    async fn handle_search_response(response: Response) -> Result<Show, Error> {
        match response.status() {
            StatusCode::OK => {
                debug!("response is ok, parsing show");
                let show = deserialize_response(response).await?;
                debug!(?show, "finished parsing show");

                Ok(show)
            }
            StatusCode::NOT_FOUND => {
                debug!("show not found");
                Err(Error::NotFound)
            }
            status => {
                error!("unexpected response status: {status}");
                Err(Error::UnexpectedResponse)
            }
        }
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
            episode.airstamp.map_or_else(
                || "???".to_string(),
                |airstamp| {
                    let dt = airstamp - now;
                    duration_in_words(dt)
                },
            )
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
            Error::Request(_) => "Failed to fetch show information".to_string(),
            Error::Deserialize(_) => "Failed to parse show information".to_string(),
            Error::UnexpectedResponse => "Received unexpected response from API".to_string(),
        };

        Self::build_formatted_message(None, &format!("Error: {content}"))
    }

    /// Builds the search URL with query parameters.
    fn build_search_url(&self, query: &str) -> Url {
        let mut url = self.urls.single_search.clone();

        url.query_pairs_mut()
            .append_pair("q", query)
            .append_pair("embed", "nextepisode");
        url
    }

    /// Formats a message for display in IRC with optional prefix.
    #[allow(clippy::option_if_let_else)]
    fn build_formatted_message(prefix: Option<&str>, message: &str) -> String {
        match prefix {
            Some(name) => format!("\x0310>\x03\x02 TVmaze\x02\x0310 (\x0f{name}\x0310): {message}"),
            None => format!("\x0310>\x03\x02 TVmaze\x02\x0310: {message}"),
        }
    }
}

/// Deserializes an HTTP response into the specified type.
///
/// # Errors
///
/// Returns `Error::Request` if reading the response fails.
/// Returns `Error::Deserialize` if parsing the JSON fails.
async fn deserialize_response<T: DeserializeOwned>(response: Response) -> Result<T, Error> {
    let text = response.text().await.map_err(Error::Request)?;
    let deserializer = &mut serde_json::Deserializer::from_slice(text.as_bytes());

    serde_path_to_error::deserialize(deserializer)
        .inspect_err(|err| error!(?err, body = %text, "failed to parse json response"))
        .map_err(Error::Deserialize)
}

fn duration_in_words(duration: Duration) -> String {
    let total_seconds = duration.whole_seconds();

    // Handle zero or negative durations
    if total_seconds <= 0 {
        return "0 minutes".to_string();
    }

    // Calculate time units
    let weeks = total_seconds / (7 * 24 * 60 * 60);
    let remaining_after_weeks = total_seconds % (7 * 24 * 60 * 60);
    let days = remaining_after_weeks / (24 * 60 * 60);
    let remaining_after_days = remaining_after_weeks % (24 * 60 * 60);
    let hours = remaining_after_days / (60 * 60);
    let remaining_after_hours = remaining_after_days % (60 * 60);
    let minutes = remaining_after_hours / 60;

    // Build the parts vector with non-zero units
    let mut parts = Vec::new();

    if weeks > 0 {
        parts.push(format!(
            "{} week{}",
            weeks,
            if weeks == 1 { "" } else { "s" }
        ));
    }
    if days > 0 {
        parts.push(format!("{} day{}", days, if days == 1 { "" } else { "s" }));
    }

    if hours > 0 {
        parts.push(format!(
            "{} hour{}",
            hours,
            if hours == 1 { "" } else { "s" }
        ));
    }

    if minutes > 0 {
        parts.push(format!(
            "{} minute{}",
            minutes,
            if minutes == 1 { "" } else { "s" }
        ));
    }

    // Format the output with proper grammar
    match parts.len() {
        0 => "0 minutes".to_string(),
        1 => parts[0].clone(),
        2 => format!("{} and {}", parts[0], parts[1]),
        _ => {
            let last = parts.pop().unwrap();
            format!("{}, and {}", parts.join(", "), last)
        }
    }
}
