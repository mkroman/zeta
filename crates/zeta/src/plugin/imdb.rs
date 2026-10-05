//! Expands IMDb links with title and person details, and searches IMDb.
//!
//! Links to titles and people on `imdb.com`, `www.imdb.com`, and `m.imdb.com` are looked up
//! through IMDb's internal GraphQL endpoint and replied to with formatted details about them.
//! The `!imdb <title>` command searches IMDb for the title and replies with the top match.
//!
//! The GraphQL endpoint needs no authentication; requests are localized with the
//! `user_country` and `user_language` settings, and adult titles are included in searches when
//! the `include_adult` setting is set (default). The plugin has no other settings.

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

/// The IMDb hosts whose links this plugin handles.
const URL_HOSTS: &[&str] = &["imdb.com", "m.imdb.com", "www.imdb.com"];
// The model types are re-exported as part of the module's API surface, even though the plugin
// itself only handles them by value.
#[allow(unused_imports)]
pub use {
    error::Error,
    model::{Person, SearchResult, SeriesInfo, Title},
};

use format::prefix;

/// Number of search results to request.
const SEARCH_LIMIT: usize = 5;

/// Settings for the imdb plugin, from its `[plugins.imdb]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Whether to include adult titles in search results.
    pub include_adult: bool,
    /// The country the IMDb API localises results for.
    pub user_country: String,
    /// The language the IMDb API localises results for.
    pub user_language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            include_adult: true,
            user_country: "US".to_string(),
            user_language: "en-US".to_string(),
        }
    }
}

/// The `!imdb` command.
const COMMAND: CommandSpec =
    CommandSpec::with_args::<Opts>("!imdb", "Search IMDb and post the top match");

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

    fn new(
        ctx: &Context,
        settings: &Settings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, ZetaError> {
        subscriptions
            .command(COMMAND)
            .urls(UrlScope::Hosts(URL_HOSTS));

        let client = GraphQlClient::new(settings, &ctx.config.http).map_err(plugin_err)?;

        Ok(Imdb { client })
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let Some(opts) = parse_args_or_usage::<Opts>(client, command, |line: &str| {
            format!("{} {line}", prefix())
        })? else {
            return Ok(());
        };

        let query = opts.query();
        if query.is_empty() {
            client.send_privmsg(channel, format!("{} usage: !imdb <title>", prefix()))?;
            return Ok(());
        }

        let results = match self.client.search(&query, SEARCH_LIMIT).await {
            Ok(results) => results,
            Err(err) => {
                client.send_privmsg(channel, format!("{} {err}", prefix()))?;
                return Ok(());
            }
        };

        let Some(result) = results.first() else {
            client.send_privmsg(channel, format!("{} no results", prefix()))?;
            return Ok(());
        };

        let lookup = self.client.title(&result.id).await;
        Self::reply(client, channel, lookup.map(|title| format_title(&title)))?;

        Ok(())
    }

    async fn handle_url(
        &self,
        _ctx: &Context,
        client: &Client,
        url: &UrlEvent,
    ) -> Result<(), ZetaError> {
        self.process_url(url.url(), url.channel(), client).await?;

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
            Err(err) => client.send_privmsg(channel, format!("{} {err}", prefix()))?,
        }

        Ok(())
    }

    /// Processes a URL found in a message, printing details about any IMDb resource it points
    /// at.
    ///
    /// URLs matching a filter — e.g. links to IMDb posted by another bot — are skipped.
    async fn process_url(&self, url: &Url, channel: &str, client: &Client) -> Result<(), ZetaError> {
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

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;
    use crate::config::HttpConfig;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.include_adult);
            assert_eq!(settings.user_country, "US");
            assert_eq!(settings.user_language, "en-US");
        }
        deserialize: {
            "include_adult": false,
            "user_country": "DK",
            "user_language": "da-DK",
        } assert: {
            assert!(!settings.include_adult);
            assert_eq!(settings.user_country, "DK");
            assert_eq!(settings.user_language, "da-DK");
        }
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

    /// Live smoke test against IMDb's GraphQL API, to catch contract drift that the recorded
    /// fixtures cannot. Run with `cargo test -p zeta --all-features -- --ignored imdb::`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs network access"]
    async fn live_api_contract_smoke() {
        let client = GraphQlClient::new(&Settings::default(), &HttpConfig::default()).unwrap();

        let results = client.search("peggle nights", SEARCH_LIMIT).await.unwrap();
        assert_ne!(results, Vec::new());

        // The top match for a stable query must still decode into the full model.
        let title = client.title(&results[0].id).await.unwrap();
        println!("{}", format_title(&title));
        assert_eq!(title.title.as_deref(), Some("Peggle Nights"));

        let person = client.person("nm0186505").await.unwrap();
        println!("{}", format_person(&person));
        assert_eq!(person.name.as_deref(), Some("Bryan Cranston"));

        // Unknown ids must still map to a not-found error.
        let err = client.title("tt9999999999999").await.unwrap_err();
        assert!(matches!(err, Error::NotFound));
    }
}
