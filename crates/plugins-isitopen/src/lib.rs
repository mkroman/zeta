//! Answers Danish natural-language questions about a place's opening hours.
//!
//! Messages addressed to the bot that match one of the question patterns —
//! `hvornår åbner X?`, `hvad tid lukker X?`, `er X åbent?`, `er X lukket?` — are looked up
//! through the Google Maps Places API (a text search followed by a place details call) and
//! answered in Danish: the opening or closing time of the queried weekday, whether the place
//! is currently open, 24/7 detection, or a status message when the place has no recorded
//! opening hours.
//!
//! The Google Maps API key is set in `[plugins.isitopen]`, falling back to the
//! `GOOGLE_MAPS_API_KEY` environment variable; a missing key fails plugin initialization and
//! the plugin is skipped at startup.

use serde::{Deserialize, Serialize};
use tracing::warn;

use zeta_http as http;
use zeta_plugin::irc::{plain_reply, send_reply};
use zeta_plugin::{Client, Context, MessageEvent, ZetaError, plugin, resolve_secret};
use zeta_utils::irc;
use zeta_utils::text;

mod model;
mod places;

use self::model::{PlaceDetails, format_time};

/// Settings for the isitopen plugin, from its `[plugins.isitopen]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The Google Maps API key.
    ///
    /// Falls back to the `GOOGLE_MAPS_API_KEY` environment variable when unset.
    pub api_key: Option<String>,
}

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
    /// The request failed, or the API response was unusable.
    #[error(transparent)]
    Api(#[from] http::ApiError),
    /// No matching place was found, or the place has no opening hours.
    #[error("place not found")]
    NotFound,
    /// The Google Maps API returned an error response.
    #[error("api error: {0}")]
    Places(String),
}

#[plugin(name = "isitopen", settings = Settings, messages)]
impl IsItOpen {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let api_key = resolve_secret(settings.api_key.as_deref(), "GOOGLE_MAPS_API_KEY")?;
        let client = ctx.http().clone();

        Ok(Self { client, api_key })
    }

    #[handler(message)]
    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        event: &MessageEvent,
    ) -> Result<(), ZetaError> {
        let current_nickname = client.current_nickname();

        // Check if the message is addressed to the bot
        if let Some(msg) = irc::strip_nick_prefix(event.text(), current_nickname)
            && let Some(nick) = event.sender().map(|sender| sender.nick)
        {
            self.process_query(event.channel(), nick, msg, client)
                .await?;
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
        let Some((action, place_name)) = parse_query(query) else {
            return Ok(());
        };

        let message = match self.find_place(place_name).await {
            Ok(place) => match action {
                QueryAction::OpeningTime => Self::format_opening_time(&place, nick),
                QueryAction::ClosingTime => Self::format_closing_time(&place, nick),
                QueryAction::IsOpen => Self::format_is_open(&place, nick),
                QueryAction::IsClosed => Self::format_is_closed(&place, nick),
            },
            Err(Error::NotFound) => plain_reply("place not found"),
            Err(e) => {
                warn!(exception.message = ?e, "isitopen error");
                plain_reply(e)
            }
        };

        send_reply(client, channel, message)?;

        Ok(())
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
            let closing = place.closing_time_or_unknown(now);
            let opening = place.opening_time_or_unknown(now);
            format!(
                "{nick}: \x02{name}\x02 lukker kl. \x02{closing}\x02 - de åbnede kl. \x02{opening}\x02"
            )
        } else {
            let (open_time, close_time) = place.open_and_close_time(now);

            if let (Some(open), Some(close)) = (open_time, close_time) {
                let now_time = now.time();
                let closing_str = format_time(close);
                let opening_str = format_time(open);

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
            let opening = place.opening_time_or_unknown(now);
            format!("{nick}: ja, \x02{name}\x02 åbnede kl. \x02{opening}\x02 i dag")
        } else {
            let (open_time, close_time) = place.open_and_close_time(now);

            if let (Some(open), Some(_close)) = (open_time, close_time) {
                let now_time = now.time();
                let opening_str = format_time(open);

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
            let opening = place.opening_time_or_unknown(now);
            let closing = place.closing_time_or_unknown(now);
            format!(
                "{nick}: nej, \x02{name}\x02 åbnede kl. \x02{opening}\x02 og lukker kl. \x02{closing}\x02 i dag"
            )
        } else {
            let (open_time, _close_time) = place.open_and_close_time(now);

            open_time.map_or_else(|| format!("{nick}: ja, \x02{name}\x02 har lukket for i dag"), |open| {
                let now_time = now.time();
                let opening_str = format_time(open);

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

/// Parses a Danish natural-language question about a place's opening hours.
fn parse_query(query: &str) -> Option<(QueryAction, &str)> {
    let query = query.strip_suffix('?')?;

    for prefix in ["hvornår åbner ", "hvad tid åbner "] {
        if let Some(place) = text::strip_prefix_ignore_case(query, prefix) {
            return Some((QueryAction::OpeningTime, place));
        }
    }

    for prefix in ["hvornår lukker ", "hvad tid lukker "] {
        if let Some(place) = text::strip_prefix_ignore_case(query, prefix) {
            return Some((QueryAction::ClosingTime, place));
        }
    }

    for prefix in ["har ", "er "] {
        if let Some(rest) = text::strip_prefix_ignore_case(query, prefix) {
            for suffix in [" åbent", " åben"] {
                if let Some(place) = text::strip_suffix_ignore_case(rest, suffix) {
                    return Some((QueryAction::IsOpen, place));
                }
            }

            if let Some(place) = text::strip_suffix_ignore_case(rest, " lukket") {
                return Some((QueryAction::IsClosed, place));
            }
        }
    }

    None
}

/// The question intent parsed from a message.
#[derive(Debug, PartialEq, Eq)]
enum QueryAction {
    OpeningTime,
    ClosingTime,
    IsOpen,
    IsClosed,
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.api_key.is_none());
        }
        deserialize: {
            "api_key": "secret",
        } assert: {
            assert_eq!(settings.api_key.as_deref(), Some("secret"));
        }
    }

    #[test]
    fn parses_danish_opening_hour_questions() {
        let cases = [
            (
                "hvornår åbner Netto?",
                Some((QueryAction::OpeningTime, "Netto")),
            ),
            (
                "HVAD TID ÅBNER Café?",
                Some((QueryAction::OpeningTime, "Café")),
            ),
            (
                "hvornår lukker Netto?",
                Some((QueryAction::ClosingTime, "Netto")),
            ),
            (
                "hvad tid lukker Netto?",
                Some((QueryAction::ClosingTime, "Netto")),
            ),
            ("har Netto åbent?", Some((QueryAction::IsOpen, "Netto"))),
            ("er Netto åben?", Some((QueryAction::IsOpen, "Netto"))),
            ("er Netto lukket?", Some((QueryAction::IsClosed, "Netto"))),
            ("er  åbent?", Some((QueryAction::IsOpen, ""))),
            ("er  lukket?", Some((QueryAction::IsClosed, ""))),
            (
                "hvornår åbner Netto??",
                Some((QueryAction::OpeningTime, "Netto?")),
            ),
        ];

        for (query, expected) in cases {
            assert_eq!(parse_query(query), expected, "{query}");
        }
    }

    #[test]
    fn rejects_unmatched_opening_hour_questions() {
        for query in [
            "hvornår åbner Netto",
            "hvornår åbner Netto? ",
            "er Netto åben",
            "hvor er Netto?",
            "",
            "?",
        ] {
            assert_eq!(parse_query(query), None, "{query}");
        }
    }
}
