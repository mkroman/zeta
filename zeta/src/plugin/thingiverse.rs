//! Thingiverse integration plugin.
//!
//! This plugin detects Thingiverse URLs in messages and fetches information
//! about the linked "thing" using the Thingiverse API.

use std::env;
use std::fmt::{self, Display};

use num_format::{Locale, ToFormattedString};
use regex::Regex;
use reqwest::header::AUTHORIZATION;
use serde::Deserialize;
use tracing::{debug, warn};
use url::Url;

use crate::{
    http,
    plugin::{self, prelude::*},
};

const API_BASE_URL: &str = "https://api.thingiverse.com";

/// Plugin for handling Thingiverse URLs.
pub struct Thingiverse {
    /// HTTP client for API requests.
    client: reqwest::Client,
    /// Thingiverse App Token.
    app_token: String,
    /// Regex for parsing thing IDs from URL paths.
    path_regex: Regex,
}

/// Errors that can occur during plugin execution.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("resource not found")]
    NotFound,
    #[error("api error: {0}")]
    Api(String),
}

/// Represents a "Thing" (3D model) from the Thingiverse API.
#[derive(Debug, Deserialize)]
struct Thing {
    /// The title of the thing.
    name: String,
    /// The user who created the thing.
    creator: Creator,
    /// Whether the thing is a work-in-progress.
    is_wip: usize,
    /// Whether the thing has been featured.
    is_featured: Option<bool>,
    /// Number of likes.
    like_count: u64,
    /// Number of downloads.
    download_count: u64,
    /// Number of times collected.
    collect_count: u64,
}

/// Represents the creator of a Thing.
#[derive(Debug, Deserialize)]
struct Creator {
    /// The username of the creator.
    name: String,
}

#[async_trait]
impl Plugin<Context> for Thingiverse {
    fn new(_ctx: &Context) -> Self {
        let app_token = env::var("THINGIVERSE_APP_TOKEN")
            .expect("missing THINGIVERSE_APP_TOKEN environment variable");
        let client = http::build_client();
        // Regex to match /thing:<id>
        let path_regex = Regex::new(r"^/thing:(?P<id>\d+)/?$").expect("invalid regex");

        Self {
            client,
            app_token,
            path_regex,
        }
    }

    fn name() -> Name {
        Name::from("thingiverse")
    }

    fn author() -> Author {
        Author::from("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version::from("1.0")
    }

    async fn handle_message(
        &self,
        _ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(urls) = plugin::extract_urls(user_message)
        {
            for url in urls {
                if let Some(host) = url.host_str()
                    && (host == "thingiverse.com" || host == "www.thingiverse.com")
                {
                    self.process_url(&url, channel, client).await?;
                }
            }
        }

        Ok(())
    }
}

impl Thingiverse {
    /// Processes a single Thingiverse URL.
    ///
    /// Checks if the URL path matches the expected Thingiverse pattern, extracts the ID,
    /// and fetches the data.
    async fn process_url(
        &self,
        url: &Url,
        channel: &str,
        client: &Client,
    ) -> Result<(), ZetaError> {
        // Extract ID from path
        if let Some(captures) = self.path_regex.captures(url.path())
            && let Some(id_match) = captures.name("id")
        {
            let thing_id = id_match.as_str();
            debug!(%thing_id, "fetching thingiverse thing");

            match self.fetch_thing(thing_id).await {
                Ok(thing) => {
                    client.send_privmsg(channel, format_irc_output(&thing.to_string()))?;
                }
                Err(Error::NotFound) => {
                    client.send_privmsg(channel, format_irc_output("Thing not found"))?;
                }
                Err(e) => {
                    warn!(error = ?e, "thingiverse api error");
                    client.send_privmsg(channel, format_irc_output(&format!("http error: {e}")))?;
                }
            }
        }

        Ok(())
    }

    /// Fetches details about a specific thing by ID from the Thingiverse API.
    async fn fetch_thing(&self, id: &str) -> Result<Thing, Error> {
        let url = format!("{API_BASE_URL}/things/{id}/");

        let response = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", self.app_token))
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::NotFound);
        }

        if !response.status().is_success() {
            return Err(Error::Api(response.status().to_string()));
        }

        response.json().await.map_err(Error::from)
    }
}

impl Display for Thing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = &self.name;
        let creator = &self.creator.name;

        let type_desc = if self.is_wip == 1 {
            "\x0f work in progress\x0310"
        } else if self.is_featured.unwrap_or(false) {
            " \x0ffeatured\x0310"
        } else {
            " thing"
        };

        let likes = self.like_count.to_formatted_string(&Locale::en);
        let like_noun = if self.like_count == 1 {
            "like"
        } else {
            "likes"
        };

        let downloads = self.download_count.to_formatted_string(&Locale::en);
        let dl_noun = if self.download_count == 1 {
            "download"
        } else {
            "downloads"
        };

        write!(
            f,
            "“\x0f{name}\x0310” is a{type_desc} created by\x0f {creator}\x0310 with\x0f {likes}\x0310 {like_noun}, \x0f{downloads}\x0310 {dl_noun}"
        )?;

        if self.collect_count > 0 {
            let collects = self.collect_count.to_formatted_string(&Locale::en);
            let coll_noun = if self.collect_count == 1 {
                "collection"
            } else {
                "collections"
            };
            write!(f, " and is part of\x0f {collects}\x0310 {coll_noun}")?;
        }

        Ok(())
    }
}

/// Wraps a message in the standard Zeta plugin prefix.
fn format_irc_output(message: &str) -> String {
    format!("\x0310>\x0F \x02Thingiverse:\x02\x0310 {message}")
}
