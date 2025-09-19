use async_trait::async_trait;
use irc::client::Client;
use irc::proto::{Command, Message};
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use time::Duration;
use tracing::{debug, error, info};

use crate::Error as ZetaError;
use crate::command::Command as ZetaCommand;
use crate::plugin;

use super::{Author, Name, Plugin, Version};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("could not deserialize response: {0}")]
    Deserialize(#[source] serde_path_to_error::Error<serde_json::Error>),
    #[error("request error: {0}")]
    Reqwest(#[source] reqwest::Error),
    #[error("no results when searching for show")]
    ShowNotFound,
    #[error("http error: {0}")]
    Http(#[source] reqwest::Error),
}

pub struct Tvmaze {
    client: reqwest::Client,
    command: ZetaCommand,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Show {
    /// The unique TVmaze id of the show.
    id: u64,
    /// The URL to the shows details page.
    url: String,
    /// The name of the show.
    name: String,
    /// The type of show, e.g. "Scripted".
    r#type: Option<String>,
    /// The main language of the show.
    language: Option<String>,
    /// List of genres of the show.
    genres: Vec<String>,
    /// The status of the show.
    status: String,
    /// The runtime of episodes in minutes.
    runtime: Option<u64>,
    /// The average runtime of episodes in minutes.
    #[serde(rename = "averageRuntime")]
    average_runtime: u64,
    /// The date when the show premiered.
    premiered: Option<String>,
    /// The date when the show ended.
    ended: Option<String>,
    /// URL to an official site.
    #[serde(rename = "officialSite")]
    official_site: String,
    /// IDs to external services.
    externals: Option<Externals>,
    /// Optional embedded response data.
    #[serde(rename = "_embedded")]
    embedded: Option<Embedded>,
}

/// External IDs
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Externals {
    tvrage: Option<u64>,
    thetvdb: Option<u64>,
    /// IMDb movie id.
    imdb: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Embedded {
    /// Details about the next episode, if available and embedded.
    #[serde(rename = "nextepisode")]
    next_episode: Option<Episode>,
    /// Details about episodes, if available and embedded.
    episodes: Option<Vec<Episode>>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct Episode {
    /// The unique TVmaze id of the episode.
    pub id: u64,
    /// The name of the episode.
    pub name: String,
    /// The season number of the episode.
    pub season: u64,
    /// The episode number.
    pub number: u64,
    /// The timestamp of when the episode aired.
    #[serde(with = "time::serde::rfc3339::option")]
    pub airstamp: Option<time::OffsetDateTime>,
}

#[async_trait]
impl Plugin for Tvmaze {
    fn new() -> Self {
        Tvmaze::new()
    }

    fn name() -> Name {
        Name("tvmaze")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(args) = self.command.parse(user_message)
        {
            match self.single_search(args).await {
                Ok(show) => {
                    if let Some(episode) = show.embedded.and_then(|e| e.next_episode) {
                        let title = episode.name;
                        let season = episode.season;
                        let number = episode.number;

                        let dotiw = {
                            let now = time::OffsetDateTime::now_utc();

                            episode.airstamp.map_or_else(
                                || "???".to_string(),
                                |airstamp| {
                                    let dt = airstamp - now;
                                    duration_in_words(dt)
                                },
                            )
                        };

                        client
                            .send_privmsg(channel, formatted(Some(show.name), &format!("Next episode “\x0f{title}\x0310” (\x0f{season}x{number}\x0310) airs in\x0f {dotiw}")))
                            .map_err(ZetaError::IrcClient)?;
                    } else {
                        let name = show.name;
                        let status = show.status;

                        client
                            .send_privmsg(channel, formatted(None, &format!("\x0f{name}\x0310 is currently marked as\x0f {status}\x0310 and there is no next episode")))
                            .map_err(ZetaError::IrcClient)?;
                    }
                }
                Err(err) => {
                    client
                        .send_privmsg(channel, formatted(None, &format!("{err}")))
                        .map_err(ZetaError::IrcClient)?;
                }
            }
        }

        Ok(())
    }
}

impl Tvmaze {
    pub fn new() -> Self {
        let client = plugin::build_http_client();
        let command = ZetaCommand::new(".next");

        Tvmaze { client, command }
    }

    /// Looks up a single show using the `/singlesearch` endpoint.
    pub async fn single_search(&self, name: &str) -> Result<Show, Error> {
        let params = [("q", name), ("embed", "nextepisode")];
        let url =
            Url::parse_with_params("https://api.tvmaze.com/singlesearch/shows", &params).unwrap();
        debug!(%url, "requesting single search");
        let req = self.client.get(url).send().await.map_err(Error::Reqwest)?;

        match req.error_for_status() {
            Ok(response) => {
                debug!("response is ok, parsing show");

                let text = response.text().await.map_err(Error::Reqwest)?;
                let jd = &mut serde_json::Deserializer::from_str(&text);
                let show: Show = serde_path_to_error::deserialize(jd)
                    .inspect_err(|err| error!(?err, %text, "could not parse show response"))
                    .map_err(Error::Deserialize)?;

                debug!(?show, "finished parsing show");

                Ok(show)
            }
            Err(err) if err.status() == Some(StatusCode::NOT_FOUND) => {
                info!(%name, "show not found");

                Err(Error::ShowNotFound)
            }
            Err(err) => Err(Error::Http(err)),
        }
    }
}

fn formatted(prefix: Option<String>, message: &String) -> String {
    if let Some(prefix) = prefix {
        return format!("\x0310>\x03\x02 TVmaze\x02\x0310 (\x0f{prefix}\x0310): {message}");
    }

    format!("\x0310>\x03\x02 TVmaze\x02\x0310: {message}")
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
