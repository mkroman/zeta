
use std::sync::OnceLock;

use regex::Regex;
use tracing::{debug, warn};

use crate::{http, plugin::prelude::*};

mod hours;
mod types;

use types::{PlaceDetails, PlaceDetailsResponse, PlaceSearchResponse};

const API_BASE_URL: &str = "https://maps.googleapis.com";

static RE_OPENING_TIME: OnceLock<Regex> = OnceLock::new();
static RE_CLOSING_TIME: OnceLock<Regex> = OnceLock::new();
static RE_IS_OPEN: OnceLock<Regex> = OnceLock::new();
static RE_IS_CLOSED: OnceLock<Regex> = OnceLock::new();

/// Plugin that allows users to query opening hours for places using the Google Maps API.
///
/// It supports natural language queries in Danish, such as "hvornår åbner X?" or "er X åben?".
pub struct IsItOpen {
    client: reqwest::Client,
    api_key: String,
}

/// Errors that can occur during plugin execution.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("place not found")]
    NotFound,
    #[error("api error: {0}")]
    Api(String),
}

#[async_trait]
impl Plugin<Context> for IsItOpen {
    fn new(_ctx: &Context) -> Result<Self, ZetaError> {
        let api_key = require_env("GOOGLE_MAPS_API_KEY")?;
        let client = http::build_client();

        // Initialize regexes (case insensitive)
        let _ = RE_OPENING_TIME.get_or_init(|| {
            Regex::new(r"(?i)^(?:hvornår|hvad tid) åbner (?P<place>.*?)\?$").unwrap()
        });
        let _ = RE_CLOSING_TIME.get_or_init(|| {
            Regex::new(r"(?i)^(?:hvornår|hvad tid) lukker (?P<place>.*?)\?$").unwrap()
        });
        let _ = RE_IS_OPEN.get_or_init(|| {
            Regex::new(r"(?i)^(?:har|er) (?P<place>.*?) (?:åbent|åben)\?$").unwrap()
        });
        let _ = RE_IS_CLOSED
            .get_or_init(|| Regex::new(r"(?i)^(?:har|er) (?P<place>.*?) lukket\?$").unwrap());

        Ok(IsItOpen { client, api_key })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "isitopen".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref inner_message) = message.command {
            let current_nickname = client.current_nickname();

            // Check if the message is addressed to the bot
            if let Some(msg) = strip_nick_prefix(inner_message, current_nickname)
                && let Some(nick) = message.source_nickname()
            {
                self.process_query(channel, nick, msg, client).await?;
            }
        }
        Ok(())
    }
}

impl IsItOpen {
    /// Determines the type of query and processes it.
    async fn process_query(
        &self,
        channel: &str,
        nick: &str,
        query: &str,
        client: &Client,
    ) -> Result<(), ZetaError> {
        let mut place_name = None;
        let mut action = QueryAction::None;

        // Determine intent based on regex matches
        if let Some(caps) = RE_OPENING_TIME.get().unwrap().captures(query) {
            place_name = Some(caps["place"].to_string());
            action = QueryAction::OpeningTime;
        } else if let Some(caps) = RE_CLOSING_TIME.get().unwrap().captures(query) {
            place_name = Some(caps["place"].to_string());
            action = QueryAction::ClosingTime;
        } else if let Some(caps) = RE_IS_OPEN.get().unwrap().captures(query) {
            place_name = Some(caps["place"].to_string());
            action = QueryAction::IsOpen;
        } else if let Some(caps) = RE_IS_CLOSED.get().unwrap().captures(query) {
            place_name = Some(caps["place"].to_string());
            action = QueryAction::IsClosed;
        }

        if let Some(place_name) = place_name {
            match self.find_place(&place_name).await {
                Ok(place) => {
                    let message = match action {
                        QueryAction::OpeningTime => Self::format_opening_time(&place, nick),
                        QueryAction::ClosingTime => Self::format_closing_time(&place, nick),
                        QueryAction::IsOpen => Self::format_is_open(&place, nick),
                        QueryAction::IsClosed => Self::format_is_closed(&place, nick),
                        QueryAction::None => return Ok(()),
                    };
                    client.send_privmsg(channel, &message)?;
                }
                Err(Error::NotFound) => {
                    client.send_privmsg(channel, formatted("Error: place not found"))?;
                }
                Err(e) => {
                    warn!(?e, "isitopen error");
                    client.send_privmsg(channel, formatted(&format!("Error: {e}")))?;
                }
            }
        }

        Ok(())
    }

    /// Performs a two-step search: first finding the Place ID, then fetching details.
    async fn find_place(&self, query: &str) -> Result<PlaceDetails, Error> {
        debug!(%query, "searching for place");

        let search_url = format!("{API_BASE_URL}/maps/api/place/textsearch/json");
        let params = [("query", query), ("key", &self.api_key)];

        let response = self.client.get(&search_url).query(&params).send().await?;
        let search_res: PlaceSearchResponse = response.json().await?;

        if search_res.status != "OK" && search_res.status != "ZERO_RESULTS" {
            return Err(Error::Api(search_res.status));
        }

        let place_id = search_res
            .results
            .first()
            .ok_or(Error::NotFound)?
            .place_id
            .clone();

        debug!(%place_id, "fetching place details");

        let details_url = format!("{API_BASE_URL}/maps/api/place/details/json");
        let details_params = [("placeid", &place_id), ("key", &self.api_key)];

        let response = self
            .client
            .get(&details_url)
            .query(&details_params)
            .send()
            .await?;
        let details_res: PlaceDetailsResponse = response.json().await?;

        if details_res.status != "OK" {
            return Err(Error::Api(details_res.status));
        }

        Ok(details_res.result)
    }

    fn format_opening_time(place: &PlaceDetails, nick: &str) -> String {
        let name = &place.name;
        let now = place.local_now();

        if place.is_always_open() {
            format!("{nick}: \x02{name}\x02 har døgnåbent")
        } else if place.is_open_now() {
            place.opening_time(now).map_or_else(|| format!("{nick}: \x02{name}\x02 har allerede åbent"), |opening_time| format!(
                    "{nick}: \x02{name}\x02 har allerede åbent - de åbnede kl. \x02{opening_time}\x02"
                ))
        } else if let Some(opening_time) = place.opening_time(now) {
            format!("{nick}: \x02{name}\x02 åbner kl. \x02{opening_time}\x02")
        } else {
            format!("{nick}: pas - \x02{name}\x02 har ikke nogen åbningstid")
        }
    }

    fn format_closing_time(place: &PlaceDetails, nick: &str) -> String {
        let name = &place.name;
        let now = place.local_now();

        if place.is_always_open() {
            format!("{nick}: \x02{name}\x02 har døgnåbent")
        } else if place.is_open_now() {
            let closing = place
                .closing_time(now)
                .unwrap_or_else(|| "ukendt tid".to_string());
            let opening = place
                .opening_time(now)
                .unwrap_or_else(|| "ukendt tid".to_string());
            format!(
                "{nick}: \x02{name}\x02 lukker kl. \x02{closing}\x02 - de åbnede kl. \x02{opening}\x02"
            )
        } else {
            let (open_time, close_time) = place.open_and_close_time(now);

            if let (Some(open), Some(close)) = (open_time, close_time) {
                let now_time = now.time();
                let closing_str = format!("{:02}:{:02}", close.hour(), close.minute());
                let opening_str = format!("{:02}:{:02}", open.hour(), open.minute());

                if open >= now_time {
                    format!(
                        "{nick}: \x02{name}\x02 lukker kl. \x02{closing_str}\x02, men de har ikke åbent endnu - de åbner først kl. \x02{opening_str}\x02"
                    )
                } else {
                    format!("{nick}: \x02{name}\x02 har lukket for resten af dagen")
                }
            } else {
                format!("{nick}: pas - \x02{name}\x02 har ikke nogen lukketid")
            }
        }
    }

    fn format_is_open(place: &PlaceDetails, nick: &str) -> String {
        let name = &place.name;
        let now = place.local_now();

        if place.is_always_open() {
            format!("{nick}: ja, \x02{name}\x02 har døgnåbent")
        } else if place.is_open_now() {
            let opening = place
                .opening_time(now)
                .unwrap_or_else(|| "ukendt tid".to_string());
            format!("{nick}: ja, \x02{name}\x02 åbnede kl. \x02{opening}\x02 i dag")
        } else {
            let (open_time, close_time) = place.open_and_close_time(now);

            if let (Some(open), Some(_close)) = (open_time, close_time) {
                let now_time = now.time();
                let opening_str = format!("{:02}:{:02}", open.hour(), open.minute());

                if open >= now_time {
                    format!(
                        "{nick}: nej, \x02{name}\x02 har lukket, men de åbner kl. \x02{opening_str}\x02"
                    )
                } else {
                    format!("{nick}: nej, \x02{name}\x02 har lukket for i dag")
                }
            } else {
                format!("{nick}: pas - \x02{name}\x02 har ikke nogen åben- og lukketid")
            }
        }
    }

    fn format_is_closed(place: &PlaceDetails, nick: &str) -> String {
        let name = &place.name;
        let now = place.local_now();

        if place.is_always_open() {
            format!("{nick}: nej, \x02{name}\x02 har døgnåbent")
        } else if place.is_open_now() {
            let opening = place
                .opening_time(now)
                .unwrap_or_else(|| "ukendt tid".to_string());
            let closing = place
                .closing_time(now)
                .unwrap_or_else(|| "ukendt tid".to_string());
            format!(
                "{nick}: nej, \x02{name}\x02 åbnede kl. \x02{opening}\x02 og lukker kl. \x02{closing}\x02 i dag"
            )
        } else {
            let (open_time, _close_time) = place.open_and_close_time(now);

            open_time.map_or_else(|| format!("{nick}: ja, \x02{name}\x02 har lukket for i dag"), |open| {
                let now_time = now.time();
                let opening_str = format!("{:02}:{:02}", open.hour(), open.minute());

                if open >= now_time {
                    format!(
                        "{nick}: ja, \x02{name}\x02 har lukket, men de åbner kl. \x02{opening_str}\x02"
                    )
                } else {
                    format!("{nick}: ja, \x02{name}\x02 har lukket for i dag")
                }
            })
        }
    }
}

/// Helper enum to map regex matches to actions.
enum QueryAction {
    None,
    OpeningTime,
    ClosingTime,
    IsOpen,
    IsClosed,
}

/// Applies IRC teal color formatting.
fn formatted(s: &str) -> String {
    format!("\x0310{s}")
}

/// Helper to strip the bot's nickname from the message start.
fn strip_nick_prefix<'a>(s: &'a str, current_nickname: &'a str) -> Option<&'a str> {
    s.strip_prefix(current_nickname).and_then(|s| {
        if s.starts_with(", ") || s.starts_with(": ") {
            Some(&s[2..])
        } else {
            None
        }
    })
}
