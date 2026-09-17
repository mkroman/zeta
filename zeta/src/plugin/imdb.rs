#![allow(clippy::doc_markdown)]

//! IMDb integration plugin.
//!
//! Monitors IRC messages for links to IMDb resources and prints details about them, and handles the
//! `!imdb <title>` command for searching IMDb.

use ::url::Url;
use argh::{ArgsInfo, FromArgs};
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::plugin::prelude::*;

mod client;
mod error;
mod format;
mod model;
mod url;

pub use client::GraphQlClient;
pub use format::{format_person, format_title};
pub use url::{Link, classify_imdb_url};
// The model types are re-exported as part of the module's API surface, even though the plugin
// itself only handles them by value.
#[allow(unused_imports)]
pub use {
    error::Error,
    model::{Person, SearchResult, SeriesInfo, Title},
};

use format::PREFIX;

/// Number of search results to request.
const SEARCH_LIMIT: usize = 5;

/// Settings for the imdb plugin, from its `[plugins.imdb]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Whether to include adult titles in search results.
    #[serde(default = "default_include_adult")]
    pub include_adult: bool,
    /// The country the IMDb API localises results for.
    #[serde(default = "default_user_country")]
    pub user_country: String,
    /// The language the IMDb API localises results for.
    #[serde(default = "default_user_language")]
    pub user_language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            include_adult: default_include_adult(),
            user_country: default_user_country(),
            user_language: default_user_language(),
        }
    }
}

/// Returns whether adult titles are included by default.
const fn default_include_adult() -> bool {
    true
}

/// Returns the default country the IMDb API localises results for.
fn default_user_country() -> String {
    "US".to_string()
}

/// Returns the default language the IMDb API localises results for.
fn default_user_language() -> String {
    "en-US".to_string()
}

/// The `!imdb` command.
const COMMAND: PluginCommand =
    PluginCommand::with_args::<Opts>(Prefix::new("!imdb"), "Search IMDb and post the top match");

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[COMMAND];

/// Search IMDb for a title.
#[derive(FromArgs, ArgsInfo, Debug)]
struct Opts {
    /// title to search for
    #[argh(positional, greedy)]
    query: Vec<String>,
}

impl Opts {
    /// The joined search query.
    fn query(&self) -> String {
        self.query.join(" ")
    }
}

/// IMDb plugin.
///
/// Prints details about IMDb links posted in a channel — titles as well as persons — and handles
/// the `!imdb <title>` command for searching IMDb.
pub struct Imdb {
    /// IMDb GraphQL client.
    client: GraphQlClient,
}

#[async_trait]
impl Plugin<Context> for Imdb {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client = GraphQlClient::new(settings, &ctx.config.http).map_err(plugin_err)?;

        Ok(Imdb { client })
    }

    const COMMANDS: &'static [PluginCommand] = COMMANDS;

    fn url_hosts(&self) -> &'static [&'static str] {
        &["imdb.com", "m.imdb.com", "www.imdb.com"]
    }

    async fn handle_message(
        &self,
        ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        let Command::PRIVMSG(channel, _) = &message.command else {
            return Ok(());
        };

        match FilteredUrls::from_message(ctx, message) {
            Some(urls) => {
                let urls: Vec<_> = urls.collect();
                self.process_urls(&urls, channel, client).await?;
            }
            None => self.dispatch_command(ctx, client, message).await?,
        }

        Ok(())
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        channel: &str,
        command: &Prefix,
        args: &str,
    ) -> Result<(), ZetaError> {
        let opts = match command.parse_args::<Opts>(args) {
            Ok(opts) => opts,
            Err(err) => {
                client.send_privmsg(channel, err.to_string())?;
                return Ok(());
            }
        };

        let query = opts.query();
        if query.is_empty() {
            client.send_privmsg(channel, format!("{PREFIX} usage: !imdb <title>"))?;
            return Ok(());
        }

        let results = match self.client.search(&query, SEARCH_LIMIT).await {
            Ok(results) => results,
            Err(err) => {
                client.send_privmsg(channel, format!("{PREFIX} {err}"))?;
                return Ok(());
            }
        };

        let Some(result) = results.first() else {
            client.send_privmsg(channel, format!("{PREFIX} no results"))?;
            return Ok(());
        };

        let lookup = self.client.title(&result.id).await;
        Self::reply(client, channel, lookup.map(|title| format_title(&title)))?;

        Ok(())
    }
}

impl Imdb {
    /// Sends the result of an IMDb lookup to `channel`, formatting errors with the IMDb prefix.
    fn reply(
        client: &Client,
        channel: &str,
        result: Result<String, Error>,
    ) -> Result<(), ZetaError> {
        match result {
            Ok(message) => client.send_privmsg(channel, message)?,
            Err(err) => client.send_privmsg(channel, format!("{PREFIX} {err}"))?,
        }

        Ok(())
    }

    /// Processes URLs found in a message, printing details about any IMDb resources.
    ///
    /// URLs matching a filter — e.g. links to IMDb posted by another bot — are skipped.
    async fn process_urls(
        &self,
        urls: &[Url],
        channel: &str,
        client: &Client,
    ) -> Result<(), ZetaError> {
        for url in urls {
            match classify_imdb_url(url) {
                Some(Link::Title(id)) => {
                    debug!(%id, "fetching details for posted title link");

                    let lookup = self.client.title(&id).await;
                    Self::reply(client, channel, lookup.map(|title| format_title(&title)))?;
                }
                Some(Link::Name(id)) => {
                    debug!(%id, "fetching details for posted person link");

                    let lookup = self.client.person(&id).await;
                    Self::reply(client, channel, lookup.map(|person| format_person(&person)))?;
                }
                None => {}
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HttpConfig;

    #[test]
    fn default_settings() {
        let settings = Settings::default();

        assert!(settings.include_adult);
        assert_eq!(settings.user_country, "US");
        assert_eq!(settings.user_language, "en-US");
    }

    #[test]
    fn settings_deserialize() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "include_adult": false,
            "user_country": "DK",
            "user_language": "da-DK",
        }))
        .expect("could not deserialize settings");

        assert!(!settings.include_adult);
        assert_eq!(settings.user_country, "DK");
        assert_eq!(settings.user_language, "da-DK");
    }

    #[test]
    fn parses_multi_word_queries() {
        let opts: Opts = COMMAND.parse_args("peggle nights").unwrap();

        assert_eq!(opts.query(), "peggle nights");
    }

    #[test]
    fn parses_quoted_queries() {
        let opts: Opts = COMMAND.parse_args(r#""the matrix""#).unwrap();

        assert_eq!(opts.query(), "the matrix");
    }

    #[test]
    fn parses_empty_query() {
        let opts: Opts = COMMAND.parse_args("").unwrap();

        assert_eq!(opts.query(), "");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires network access"]
    async fn live_search_and_title() {
        let client = GraphQlClient::new(&Settings::default(), &HttpConfig::default()).unwrap();

        let results = client.search("peggle nights", 5).await.unwrap();
        assert!(!results.is_empty());
        assert_eq!(results.first().unwrap().id, "tt14663588");

        let title = client.title("tt14663588").await.unwrap();
        println!("{}", format_title(&title));
        assert_eq!(title.title.as_deref(), Some("Peggle Nights"));
        assert_eq!(title.rating, Some(7.6));
        assert_eq!(title.genres, ["Action", "Adventure", "Family"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires network access"]
    async fn live_unknown_title_is_not_found() {
        let client = GraphQlClient::new(&Settings::default(), &HttpConfig::default()).unwrap();

        let err = client.title("tt9999999999999").await.unwrap_err();

        assert!(matches!(err, Error::NotFound));
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires network access"]
    async fn live_episode() {
        let client = GraphQlClient::new(&Settings::default(), &HttpConfig::default()).unwrap();

        let title = client.title("tt0959621").await.unwrap();
        println!("{}", format_title(&title));

        assert!(title.is_episode());
        let series = title.series.expect("series info");
        assert_eq!(series.title.as_deref(), Some("Breaking Bad"));
        assert_eq!(series.season.as_deref(), Some("1"));
        assert_eq!(series.number.as_deref(), Some("1"));
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires network access"]
    async fn live_person() {
        let client = GraphQlClient::new(&Settings::default(), &HttpConfig::default()).unwrap();

        let person = client.person("nm0186505").await.unwrap();
        println!("{}", format_person(&person));

        assert_eq!(person.name.as_deref(), Some("Bryan Cranston"));
        assert_eq!(person.birth_year, Some(1956));
        assert!(!person.known_for.is_empty());
    }
}
