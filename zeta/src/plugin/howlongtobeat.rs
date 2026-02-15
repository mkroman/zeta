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

/// HowLongToBeat plugin.
pub struct HowLongToBeat {
    client: reqwest::Client,
    command: ZetaCommand,
    /// Cached authentication token.
    token: RwLock<Option<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("could not deserialize response: {0}")]
    Deserialize(#[source] reqwest::Error),
}

#[derive(Debug, Deserialize)]
struct InitResponse {
    token: String,
}

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

#[derive(Debug, Serialize)]
struct SearchOptions {
    games: GamesOptions,
    users: UsersOptions,
    lists: ListsOptions,
    filter: String,
    sort: u32,
    randomizer: u32,
}

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

#[derive(Debug, Serialize)]
struct RangeTime {
    min: Option<u32>,
    max: Option<u32>,
}

#[derive(Debug, Serialize)]
struct RangeYear {
    min: String,
    max: String,
}

#[derive(Debug, Serialize)]
struct GameplayOptions {
    perspective: String,
    flow: String,
    genre: String,
    difficulty: String,
}

#[derive(Debug, Serialize)]
struct UsersOptions {
    #[serde(rename = "sortCategory")]
    sort_category: String,
}

#[derive(Debug, Serialize)]
struct ListsOptions {
    #[serde(rename = "sortCategory")]
    sort_category: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    data: Vec<Game>,
}

#[allow(clippy::struct_field_names)]
#[derive(Debug, Deserialize)]
struct Game {
    game_name: String,
    /// Main Story time in seconds
    comp_main: u32,
    /// Main + Extra time in seconds
    comp_plus: u32,
    /// Completionist time in seconds
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
    fn new(_ctx: &Context) -> Self {
        let client = http::build_client();
        let command = ZetaCommand::new(".hltb");

        Self {
            client,
            command,
            token: RwLock::new(None),
        }
    }

    fn name() -> Name {
        Name::from("hltb")
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
    /// Acquires a valid auth token.
    ///
    /// If a cached token exists, it is returned. Otherwise, a new token is fetched.
    async fn get_token(&self) -> Result<String, Error> {
        // Check cache first
        if let Some(token) = self.token.read().await.as_ref() {
            return Ok(token.clone());
        }

        // Fetch new token
        self.refresh_token().await
    }

    /// Forces a refresh of the auth token from the API.
    async fn refresh_token(&self) -> Result<String, Error> {
        let url = format!("{BASE_URL}/api/search/init");
        debug!("refreshing hltb token");

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

        {
            let mut token_lock = self.token.write().await;
            *token_lock = Some(init.token.clone());
        }

        Ok(init.token)
    }

    /// Performs a search for the given game.
    ///
    /// Handles token expiration by retrying once if a 403 Forbidden is encountered.
    async fn search(&self, query: &str) -> Result<Vec<Game>, Error> {
        // First attempt
        let token = self.get_token().await?;
        match self.perform_search_request(&token, query).await {
            Ok(results) => Ok(results),
            Err(Error::Request(e)) if e.status() == Some(StatusCode::FORBIDDEN) => {
                // Token likely expired, refresh and retry once
                warn!("hltb token expired, refreshing...");
                let new_token = self.refresh_token().await?;
                self.perform_search_request(&new_token, query).await
            }
            Err(e) => Err(e),
        }
    }

    async fn perform_search_request(&self, token: &str, query: &str) -> Result<Vec<Game>, Error> {
        let url = format!("{BASE_URL}/api/search");
        let search_terms: Vec<&str> = query.split_whitespace().collect();

        let body = SearchRequest {
            search_type: "games",
            search_terms,
            search_page: 1,
            size: 20,
            search_options: SearchOptions::default(),
            use_cache: true,
        };

        let response = self
            .client
            .post(&url)
            .header(REFERER, REFERER_URL)
            .header("x-auth-token", token)
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

fn format_game(game: &Game) -> String {
    let main = format_seconds(game.comp_main);
    let main_extra = format_seconds(game.comp_plus);
    let completionist = format_seconds(game.comp_100);

    format!(
        "\x0310>\x03\x02 HLTB\x02\x0310 (\x0f{}\x0310): Main Story: \x0f{}\x0310 | Main + Extra: \x0f{}\x0310 | Completionist: \x0f{}",
        game.game_name, main, main_extra, completionist
    )
}

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
