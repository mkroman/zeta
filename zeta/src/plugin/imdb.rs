#![allow(clippy::doc_markdown)]

//! IMDb integration plugin.
//!
//! Monitors IRC messages for links to IMDb resources and prints details about them, and handles the
//! `.imdb <title>` command for searching IMDb.

use ::url::Url;
use argh::FromArgs;
use tracing::debug;

use crate::plugin::{self, prelude::*};

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

/// The `!imdb` command.
const COMMAND: Prefix = Prefix::new("!imdb");

/// Command-line options for the `.imdb` command.
#[derive(FromArgs, Debug)]
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
/// the `.imdb <title>` command for searching IMDb.
pub struct Imdb {
    /// IMDb GraphQL client.
    client: GraphQlClient,
}

#[async_trait]
impl Plugin<Context> for Imdb {
    fn new(_ctx: &Context) -> Result<Self, ZetaError> {
        let client = GraphQlClient::new().map_err(plugin_err)?;

        Ok(Imdb { client })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "imdb".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn commands(&self) -> &'static [Prefix] {
        const { &[Prefix::new(".imdb")] }
    }

    async fn handle_message(
        &self,
        ctx: &Context,
        client: &Client,
        message: &Message,
    ) -> Result<(), ZetaError> {
        let Command::PRIVMSG(ref channel, ref user_message) = message.command else {
            return Ok(());
        };

        if let Some(urls) = plugin::extract_urls(user_message) {
            self.process_urls(&urls, channel, client).await?;
        } else {
            self.dispatch_command(ctx, client, message).await?;
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
            client.send_privmsg(channel, format!("{PREFIX} usage: .imdb <title>"))?;
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
        let client = GraphQlClient::new().unwrap();

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
        let client = GraphQlClient::new().unwrap();

        let err = client.title("tt9999999999999").await.unwrap_err();

        assert!(matches!(err, Error::NotFound));
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires network access"]
    async fn live_episode() {
        let client = GraphQlClient::new().unwrap();

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
        let client = GraphQlClient::new().unwrap();

        let person = client.person("nm0186505").await.unwrap();
        println!("{}", format_person(&person));

        assert_eq!(person.name.as_deref(), Some("Bryan Cranston"));
        assert_eq!(person.birth_year, Some(1956));
        assert!(!person.known_for.is_empty());
    }
}
