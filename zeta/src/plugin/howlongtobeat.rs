use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::{
    StatusCode,
    header::{CONTENT_TYPE, REFERER},
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::{http, plugin::prelude::*};

const BASE_URL: &str = "https://howlongtobeat.com";
const REFERER_URL: &str = "https://howlongtobeat.com/";

/// The HowLongToBeat IRC plugin.
///
/// This plugin allows users to query the HowLongToBeat database
/// to retrieve average completion times for video games.
pub struct HowLongToBeat {
    /// The HTTP client used for requests.
    client: reqwest::Client,
    /// The parsed trigger command for the plugin.
    command: ZetaCommand,
    /// Cached authentication data (token and homepage key/value).
    auth: RwLock<Option<AuthData>>,
}

/// Errors that can occur during API interactions.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("could not deserialize response: {0}")]
    Deserialize(#[source] reqwest::Error),
}

/// Cached authentication credentials required by the API.
#[derive(Debug, Clone)]
struct AuthData {
    /// The standard authorization token.
    token: String,
    /// The dynamically generated homepage key.
    hp_key: String,
    /// The dynamically generated homepage value.
    hp_val: String,
}

/// The response payload received from the initialization endpoint.
#[derive(Debug, Deserialize)]
struct InitResponse {
    token: String,
    #[serde(rename = "hpKey")]
    hp_key: String,
    #[serde(rename = "hpVal")]
    hp_val: String,
}

/// The request body sent to the `/api/find` endpoint to search for games.
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
    /// Dynamically injects the homepage key/value into the root of the JSON payload.
    #[serde(flatten)]
    homepage_data: HashMap<String, String>,
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
    platform: String,
    #[serde(rename = "sortCategory")]
    sort_category: String,
    #[serde(rename = "rangeCategory")]
    range_category: String,
    #[serde(rename = "rangeTime")]
    range_time: RangeTime,
    gameplay: GameplayOptions,
    #[serde(rename = "rangeYear")]
    range_year: RangeYear,
    modifier: String,
}

/// Time range filters for completion data.
#[derive(Debug, Serialize)]
struct RangeTime {
    min: Option<u32>,
    max: Option<u32>,
}

/// Release year filters for games.
#[derive(Debug, Serialize)]
struct RangeYear {
    min: String,
    max: String,
}

/// Options filtering by gameplay styles and genres.
#[derive(Debug, Serialize)]
struct GameplayOptions {
    perspective: String,
    flow: String,
    genre: String,
    difficulty: String,
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
                platform: String::new(),
                sort_category: "popular".to_string(),
                range_category: "main".to_string(),
                range_time: RangeTime {
                    min: None,
                    max: None,
                },
                gameplay: GameplayOptions {
                    perspective: String::new(),
                    flow: String::new(),
                    genre: String::new(),
                    difficulty: String::new(),
                },
                range_year: RangeYear {
                    min: String::new(),
                    max: String::new(),
                },
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

#[async_trait]
impl Plugin<Context> for HowLongToBeat {
    fn new(_ctx: &Context) -> Result<Self, BoxError> {
        let client = http::build_client();
        let command = ZetaCommand::new(".hltb");

        Ok(Self {
            client,
            command,
            auth: RwLock::new(None),
        })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "hltb".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(query) = self.command.parse(user_message)
        {
            if query.trim().is_empty() {
                client.send_privmsg(channel, "\x0310> Usage: .hltb\x0f <game>")?;
                return Ok(());
            }

            match self.search(query).await {
                Ok(games) => {
                    if let Some(game) = games.first() {
                        let msg = format_game(game);
                        client.send_privmsg(channel, msg)?;
                    } else {
                        client.send_privmsg(channel, "\x0310> No results found")?;
                    }
                }
                Err(err) => {
                    warn!(?err, "hltb search failed");
                    client.send_privmsg(channel, format!("\x0310> Failed to fetch data: {err}"))?;
                }
            }
        }

        Ok(())
    }
}

impl HowLongToBeat {
    /// Acquires valid API authentication credentials.
    ///
    /// Returns the cached data if it exists, otherwise requests new data from the initialization endpoint.
    async fn get_auth(&self) -> Result<AuthData, Error> {
        if let Some(auth) = self.auth.read().await.as_ref() {
            return Ok(auth.clone());
        }

        self.refresh_auth().await
    }

    /// Fetches a fresh authorization token and homepage key/value pair from the API.
    async fn refresh_auth(&self) -> Result<AuthData, Error> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        let url = format!("{BASE_URL}/api/find/init?t={timestamp}");
        debug!("refreshing hltb token and homepage data");

        let response = self
            .client
            .get(&url)
            .header(REFERER, REFERER_URL)
            .send()
            .await?;

        let init: InitResponse = response
            .error_for_status()?
            .json()
            .await
            .map_err(Error::Deserialize)?;

        let auth_data = AuthData {
            token: init.token,
            hp_key: init.hp_key,
            hp_val: init.hp_val,
        };

        {
            let mut auth_lock = self.auth.write().await;
            *auth_lock = Some(auth_data.clone());
        }

        Ok(auth_data)
    }

    /// Performs a search for a specific game query.
    ///
    /// If the request returns a `403 Forbidden`, it assumes the token has expired,
    /// refreshes the credentials, and automatically retries the request once.
    async fn search(&self, query: &str) -> Result<Vec<Game>, Error> {
        let auth = self.get_auth().await?;

        match self.perform_search_request(&auth, query).await {
            Ok(results) => Ok(results),
            Err(Error::Request(e)) if e.status() == Some(StatusCode::FORBIDDEN) => {
                warn!("hltb token expired, refreshing...");
                let new_auth = self.refresh_auth().await?;
                self.perform_search_request(&new_auth, query).await
            }
            Err(e) => Err(e),
        }
    }

    /// Executes the HTTP POST request to the API with the necessary headers and payload.
    async fn perform_search_request(
        &self,
        auth: &AuthData,
        query: &str,
    ) -> Result<Vec<Game>, Error> {
        let url = format!("{BASE_URL}/api/find");
        let search_terms: Vec<&str> = query.split_whitespace().collect();

        // Inject the homepage key/value dynamically into the JSON root.
        let mut homepage_data = HashMap::new();
        homepage_data.insert(auth.hp_key.clone(), auth.hp_val.clone());

        let body = SearchRequest {
            search_type: "games",
            search_terms,
            search_page: 1,
            size: 20,
            search_options: SearchOptions::default(),
            use_cache: true,
            homepage_data,
        };

        let response = self
            .client
            .post(&url)
            .header(REFERER, REFERER_URL)
            .header("x-auth-token", &auth.token)
            .header("x-hp-key", &auth.hp_key)
            .header("x-hp-val", &auth.hp_val)
            .header(CONTENT_TYPE, "application/json")
            .json(&body)
            .send()
            .await?;

        let response_data: SearchResponse = response
            .error_for_status()?
            .json()
            .await
            .map_err(Error::Deserialize)?;

        Ok(response_data.data)
    }
}

/// Formats a parsed `Game` struct into an IRC-friendly text string.
fn format_game(game: &Game) -> String {
    let main = format_seconds(game.comp_main);
    let main_extra = format_seconds(game.comp_plus);
    let completionist = format_seconds(game.comp_100);

    format!(
        "\x0310>\x03\x02 HLTB\x02\x0310 (\x0f{}\x0310): Main Story: \x0f{}\x0310 | Main + Extra: \x0f{}\x0310 | Completionist: \x0f{}",
        game.game_name, main, main_extra, completionist
    )
}

/// Converts a duration in seconds into a human-readable hours and minutes string.
fn format_seconds(seconds: u32) -> String {
    if seconds == 0 {
        return "--".to_string();
    }

    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;

    if hours > 0 {
        if minutes > 0 {
            format!("{hours} hours {minutes} mins")
        } else {
            format!("{hours} hours")
        }
    } else {
        format!("{minutes} mins")
    }
}
