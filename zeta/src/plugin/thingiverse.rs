//! Expands Thingiverse links with details about the 3D model.
//!
//! Links to `/thing:<id>` pages on `thingiverse.com` are looked up through the Thingiverse API
//! and replied to with the thing's name, creator, and like, download, and collection counts —
//! describing work-in-progress and featured things as such. A missing thing is noted in the
//! channel.
//!
//! The API token is set in `[plugins.thingiverse]`, falling back to the `THINGIVERSE_APP_TOKEN`
//! environment variable, and is sent as a bearer token; a missing token fails plugin
//! initialization and the plugin is skipped at startup.

use std::fmt::{self, Display};

use num_format::{Locale, ToFormattedString};
use regex::Regex;
use reqwest::header::AUTHORIZATION;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use url::Url;

use crate::{http, plugin::prelude::*};

/// The Thingiverse hosts whose links this plugin handles.
const URL_HOSTS: &[&str] = &["thingiverse.com", "www.thingiverse.com"];

const API_BASE_URL: &str = "https://api.thingiverse.com";

/// Settings for the thingiverse plugin, from its `[plugins.thingiverse]` configuration section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Settings {
    /// The Thingiverse app token.
    ///
    /// Falls back to the `THINGIVERSE_APP_TOKEN` environment variable when unset.
    #[serde(default)]
    pub api_key: Option<String>,
}

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
    /// Sending the HTTP request failed.
    #[error("request error: {0}")]
    Request(reqwest::Error),
    /// The linked thing does not exist.
    #[error("resource not found")]
    NotFound,
    /// The Thingiverse API returned an error response.
    #[error(transparent)]
    Api(#[from] http::ApiError),
}

impl From<reqwest::Error> for Error {
    /// Strips the URL from `error` before wrapping it — a logged request URL can carry a
    /// credential in its query string.
    fn from(error: reqwest::Error) -> Self {
        Self::Request(error.without_url())
    }
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
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.urls(UrlScope::Hosts(URL_HOSTS));

        let app_token = resolve_secret(settings.api_key.as_deref(), "THINGIVERSE_APP_TOKEN")?;
        let client = http::build_client(&ctx.config.http);
        // Regex to match /thing:<id>
        let path_regex = Regex::new(r"^/thing:(?P<id>\d+)/?$").expect("invalid regex");

        Ok(Self {
            client,
            app_token,
            path_regex,
        })
    }

    async fn handle_url(&self, _ctx: &Context, client: &Client, url: &UrlEvent) -> Result<(), ZetaError> {
        self.process_url(url.url(), url.channel(), client).await?;

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
                    client.send_privmsg(channel, reply("Thingiverse", thing.to_string()))?;
                }
                Err(Error::NotFound) => {
                    client.send_privmsg(channel, reply("Thingiverse", "Thing not found"))?;
                }
                Err(e) => {
                    warn!(error = ?e, "thingiverse api error");
                    client.send_privmsg(channel, reply("Thingiverse", e))?;
                }
            }
        }

        Ok(())
    }

    /// Fetches details about a specific thing by ID from the Thingiverse API.
    async fn fetch_thing(&self, id: &str) -> Result<Thing, Error> {
        let url = format!("{API_BASE_URL}/things/{id}/");

        let request = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", self.app_token));
        let response = http::send(request).await?;

        http::parse_response_or_404(response, Error::NotFound).await
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
