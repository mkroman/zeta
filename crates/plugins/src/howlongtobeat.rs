//! Looks up how long games take to complete on HowLongToBeat.
//!
//! The `.hltb <game>` command searches HowLongToBeat and replies with the top match's
//! completion times in words: Main Story, Main + Extra, and Completionist — each rendered as
//! `--` when the game has no recorded time. No matches and fetch errors are sent as a notice.
//!
//! The site's unauthenticated API requires a short-lived search token: the plugin bootstraps it
//! from the init endpoint, caches it in memory, sends it as the `x-auth-token` header, and on a
//! `403 Forbidden` refreshes the token and retries the search once. The token binds the caller's
//! IP address and User-Agent, so the init and search requests must go through one client.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::{
    StatusCode,
    header::{CONTENT_TYPE, REFERER},
};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::prelude::*;
use zeta_cache::TtlCache;
use zeta_http as http;
use zeta_utils::duration::{HOURS_AND_MINUTES, words};

const BASE_URL: &str = "https://howlongtobeat.com";
const REFERER_URL: &str = "https://howlongtobeat.com/";

/// How long the cached search token stays in use before a new one is requested.
///
/// The token is valid until the API starts rejecting it with a `403 Forbidden`, which triggers
/// an unconditional refresh, so the TTL only bounds how long an outdated token can survive
/// without triggering the rejection path.
const AUTH_TTL: Duration = Duration::from_hours(1);

/// The HowLongToBeat IRC plugin.
///
/// This plugin allows users to query the HowLongToBeat database
/// to retrieve average completion times for video games.
pub struct HowLongToBeat {
    /// The HTTP client used for requests.
    client: reqwest::Client,
    /// Cached search token.
    auth: TtlCache<String>,
}

/// Errors that can occur during API interactions.
pub type Error = http::ApiError;

/// The response payload received from the initialization endpoint.
#[derive(Debug, Deserialize)]
struct InitResponse {
    token: String,
}

/// The request body sent to the search endpoint, mirroring the site's own payload.
#[derive(Debug, Serialize)]
struct SearchRequest<'a> {
    #[serde(rename = "searchType")]
    search_type: &'static str,
    #[serde(rename = "searchTerms")]
    search_terms: Vec<&'a str>,
    #[serde(rename = "searchPage")]
    search_page: u32,
    size: u32,
    #[serde(rename = "searchOptions")]
    search_options: SearchOptions,
    #[serde(rename = "useCache")]
    use_cache: bool,
}

/// Comprehensive configuration options for the search query.
#[derive(Debug, Serialize)]
struct SearchOptions {
    games: GamesOptions,
    users: UsersOptions,
    lists: ListsOptions,
    filter: String,
    sort: u32,
    randomizer: u32,
}

/// Filtering and sorting options specific to game searches.
#[derive(Debug, Serialize)]
struct GamesOptions {
    #[serde(rename = "userId")]
    user_id: u32,
    platform: FacetFilter,
    #[serde(rename = "sortCategory")]
    sort_category: String,
    #[serde(rename = "rangeCategory")]
    range_category: String,
    #[serde(rename = "rangeTime")]
    range_time: RangeTime,
    gameplay: GameplayOptions,
    year: FacetFilter,
    modifier: String,
}

/// Time range filters for completion data.
///
/// The site sends `null` bounds, which the endpoint requires — omitting the keys entirely
/// rejects the request — so `None` must keep serializing as `null`.
#[derive(Debug, Serialize)]
struct RangeTime {
    min: Option<u32>,
    max: Option<u32>,
}

/// An unset search facet, matching the site's `{"mode":"include","values":[]}` payload.
///
/// `values` is a `Vec<()>` so it stays permanently empty while still serializing as `[]`.
#[derive(Debug, Serialize)]
struct FacetFilter {
    mode: &'static str,
    values: Vec<()>,
}

impl Default for FacetFilter {
    fn default() -> Self {
        Self {
            mode: "include",
            values: Vec::new(),
        }
    }
}

/// Options filtering by gameplay styles and genres.
#[derive(Debug, Serialize)]
struct GameplayOptions {
    perspective: FacetFilter,
    flow: FacetFilter,
    genre: FacetFilter,
}

/// Options filtering by user data.
#[derive(Debug, Serialize)]
struct UsersOptions {
    #[serde(rename = "sortCategory")]
    sort_category: String,
}

/// Options filtering by user lists.
#[derive(Debug, Serialize)]
struct ListsOptions {
    #[serde(rename = "sortCategory")]
    sort_category: String,
}

/// The response payload containing search results.
#[derive(Debug, Deserialize)]
struct SearchResponse {
    data: Vec<Game>,
}

/// Represents a single game result and its completion statistics.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Deserialize)]
struct Game {
    /// The name of the game.
    game_name: String,
    /// Main Story time in seconds.
    comp_main: u32,
    /// Main + Extra time in seconds.
    comp_plus: u32,
    /// Completionist (100%) time in seconds.
    comp_100: u32,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            games: GamesOptions {
                user_id: 0,
                platform: FacetFilter::default(),
                sort_category: "popular".to_string(),
                range_category: "main".to_string(),
                range_time: RangeTime {
                    min: None,
                    max: None,
                },
                gameplay: GameplayOptions {
                    perspective: FacetFilter::default(),
                    flow: FacetFilter::default(),
                    genre: FacetFilter::default(),
                },
                year: FacetFilter::default(),
                modifier: String::new(),
            },
            users: UsersOptions {
                sort_category: "postcount".to_string(),
            },
            lists: ListsOptions {
                sort_category: "follows".to_string(),
            },
            filter: String::new(),
            sort: 0,
            randomizer: 0,
        }
    }
}

#[plugin(name = "howlongtobeat")]
impl HowLongToBeat {
    // The `Plugin` trait requires the constructor to return `Result`, even when initialization
    // cannot fail.
    fn new(ctx: &Context) -> Result<Self, ZetaError> {
        let client = http::build_client(&ctx.config.http);

        Ok(Self {
            client,
            auth: TtlCache::new(AUTH_TTL),
        })
    }

    /// Replies to `.hltb` with the top match's completion times.
    #[command(".hltb", "Look up a game's completion times", event)]
    async fn lookup(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let query = command.args();

        let message = if query.trim().is_empty() {
            notice("Usage: .hltb\x0f <game>")
        } else {
            match self.search(query).await {
                Ok(games) => games
                    .first()
                    .map_or_else(|| notice("No results found"), format_game),
                Err(err) => notice(err),
            }
        };

        client.send_privmsg(channel, message)?;

        Ok(())
    }
}

impl HowLongToBeat {
    /// Acquires a valid API search token.
    ///
    /// Returns the cached token if it exists, otherwise requests a new one from the
    /// initialization endpoint.
    async fn get_auth(&self) -> Result<String, Error> {
        self.auth.get_or_refresh(|| self.fetch_auth()).await
    }

    /// Fetches a fresh search token from the initialization endpoint.
    ///
    /// The token binds the caller's IP address and User-Agent, so it is only valid for
    /// requests made through the same client.
    async fn fetch_auth(&self) -> Result<String, Error> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        let url = format!("{BASE_URL}/api/search/site/init?t={timestamp}");
        debug!("refreshing hltb search token");

        let request = self.client.get(&url).header(REFERER, REFERER_URL);
        let init: InitResponse = http::get_json(request).await?;

        Ok(init.token)
    }

    /// Performs a search for a specific game query.
    ///
    /// If the request returns a `403 Forbidden`, it assumes the token has expired,
    /// refreshes the credentials, and automatically retries the request once.
    async fn search(&self, query: &str) -> Result<Vec<Game>, Error> {
        let token = self.get_auth().await?;

        match self.perform_search_request(&token, query).await {
            Ok(results) => Ok(results),
            Err(http::ApiError::Status {
                status: StatusCode::FORBIDDEN,
                ..
            }) => {
                warn!("hltb token expired, refreshing...");
                let new_token = self.auth.force_refresh(|| self.fetch_auth()).await?;
                self.perform_search_request(&new_token, query).await
            }
            Err(e) => Err(e),
        }
    }

    /// Executes the HTTP POST request to the API with the necessary headers and payload.
    async fn perform_search_request(&self, token: &str, query: &str) -> Result<Vec<Game>, Error> {
        let url = format!("{BASE_URL}/api/search/site");
        let search_terms: Vec<&str> = query.split_whitespace().collect();

        let body = SearchRequest {
            search_type: "games",
            search_terms,
            search_page: 1,
            size: 20,
            search_options: SearchOptions::default(),
            use_cache: true,
        };

        let request = self
            .client
            .post(&url)
            .header(REFERER, REFERER_URL)
            .header("x-auth-token", token)
            .header(CONTENT_TYPE, "application/json")
            .json(&body);
        let response_data: SearchResponse = http::get_json(request).await?;

        Ok(response_data.data)
    }
}

/// Formats a parsed `Game` struct into an IRC-friendly text string.
fn format_game(game: &Game) -> String {
    let main = format_seconds(game.comp_main);
    let main_extra = format_seconds(game.comp_plus);
    let completionist = format_seconds(game.comp_100);

    reply(
        "HLTB",
        format!(
            "(\x0f{}\x0310): Main Story: \x0f{}\x0310 | Main + Extra: \x0f{}\x0310 | Completionist: \x0f{}",
            game.game_name, main, main_extra, completionist
        ),
    )
}

/// Converts a duration in seconds into words, e.g. `"2 hours and 3 minutes"`.
fn format_seconds(seconds: u32) -> String {
    if seconds == 0 {
        return "--".to_string();
    }

    words(i64::from(seconds), HOURS_AND_MINUTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_request_serializes_to_the_site_payload() {
        let body = SearchRequest {
            search_type: "games",
            search_terms: vec!["outer", "wilds"],
            search_page: 1,
            size: 20,
            search_options: SearchOptions::default(),
            use_cache: true,
        };

        // The endpoint rejects requests that omit any of these keys, e.g. `rangeTime`'s bounds
        // must be present as `null` — so the exact shape is pinned, not just the happy fields.
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            serde_json::json!({
                "searchType": "games",
                "searchTerms": ["outer", "wilds"],
                "searchPage": 1,
                "size": 20,
                "searchOptions": {
                    "games": {
                        "userId": 0,
                        "platform": {"mode": "include", "values": []},
                        "sortCategory": "popular",
                        "rangeCategory": "main",
                        "rangeTime": {"min": null, "max": null},
                        "gameplay": {
                            "perspective": {"mode": "include", "values": []},
                            "flow": {"mode": "include", "values": []},
                            "genre": {"mode": "include", "values": []}
                        },
                        "year": {"mode": "include", "values": []},
                        "modifier": ""
                    },
                    "users": {"sortCategory": "postcount"},
                    "lists": {"sortCategory": "follows"},
                    "filter": "",
                    "sort": 0,
                    "randomizer": 0
                },
                "useCache": true
            })
        );
    }

    #[test]
    fn formats_durations() {
        let cases = [
            ("missing", 0, "--"),
            ("seconds round down to minutes", 30, "0 minutes"),
            ("minutes", 45 * 60, "45 minutes"),
            ("whole hour", 3600, "1 hour"),
            (
                "hours and minutes",
                2 * 3600 + 3 * 60,
                "2 hours and 3 minutes",
            ),
            ("many hours", 100 * 3600, "100 hours"),
        ];

        for (case, seconds, expected) in cases {
            assert_eq!(format_seconds(seconds), expected, "{case}");
        }
    }

    /// Live smoke test against HowLongToBeat's search API, to catch contract drift the payload
    /// test cannot. Run with `cargo test -p plugins --all-features -- --ignored howlongtobeat::`.
    #[tokio::test]
    #[ignore = "needs network access"]
    async fn live_api_contract_smoke() {
        use zeta_plugin::config::HttpConfig;

        let plugin = HowLongToBeat {
            client: http::build_client(&HttpConfig::default()),
            auth: TtlCache::new(AUTH_TTL),
        };

        let games = plugin.search("breath of the wild").await.unwrap();
        assert!(!games.is_empty());
        assert!(
            games[0].game_name.contains("Breath of the Wild"),
            "{:?}",
            games[0].game_name
        );
        assert!(games[0].comp_main > 0);

        println!("{}", format_game(&games[0]));
    }
}
