//! Expands IMDb links with title and person details, and searches IMDb.
//!
//! Links to titles and people on `imdb.com`, `www.imdb.com`, and `m.imdb.com` are looked up
//! through IMDb's internal GraphQL endpoint and replied to with formatted details about them.
//! The `!imdb <title>` command searches IMDb for the title and replies with the top match.
//!
//! The GraphQL endpoint needs no authentication; requests are localized with the
//! `user_country` and `user_language` settings, and adult titles are included in searches when
//! the `include_adult` setting is set (default). The plugin has no other settings.

use clap::Parser;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use zeta_plugin::irc::{parse_or_usage, send_lookup_reply, send_reply};
use zeta_plugin::{
    ArgsMode, Chain, Client, CommandEvent, CommandSpec, Context, Subscriptions, UrlEvent,
    ZetaError, plugin, plugin_err,
};

mod client;
mod error;
mod format;
mod model;
mod urls;

pub use client::GraphQlClient;
pub use format::{format_person, format_title};
pub use urls::{Link, classify_imdb_url};

/// The IMDb hosts whose links this plugin handles.
const URL_HOSTS: &[&str] = &["imdb.com", "m.imdb.com", "www.imdb.com"];
pub use error::Error;
pub use model::{Person, SearchResult, SeriesInfo, Title};

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
#[derive(Parser, Debug)]
struct Opts {
    /// title to search for
    #[arg(num_args = 1.., trailing_var_arg = true)]
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

#[plugin(name = "imdb", settings = Settings, urls(hosts = URL_HOSTS))]
impl Imdb {
    fn new(
        ctx: &Context,
        settings: &Settings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, ZetaError> {
        subscriptions.command(COMMAND);

        let client = GraphQlClient::try_new(settings, ctx.config().http()).map_err(plugin_err)?;

        Ok(Self { client })
    }

    /// Replies to `!imdb` with the top title matching the query.
    #[handler(command)]
    async fn search(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let Some(opts) = parse_or_usage::<Opts>(client, command, ArgsMode::Shell, |line: &str| {
            format!("{} {line}", format::prefix())
        })?
        else {
            return Ok(());
        };

        let query = opts.query();
        if query.is_empty() {
            send_reply(
                client,
                channel,
                format!("{} usage: !imdb <title>", format::prefix()),
            )?;
            return Ok(());
        }

        let results = match self.client.search(&query, SEARCH_LIMIT).await {
            Ok(results) => results,
            Err(error) => {
                send_reply(client, channel, search_error_reply(&error))?;
                return Ok(());
            }
        };

        let Some(result) = results.first() else {
            send_reply(client, channel, format!("{} no results", format::prefix()))?;
            return Ok(());
        };

        send_lookup_reply(
            client,
            channel,
            self.client.title(&result.id),
            |title| format_title(&title),
            |error| title_error_reply(&error),
        )
        .await?;

        Ok(())
    }

    /// Expands posted IMDb links with details about the title or person they point at.
    #[handler(url)]
    async fn expand_url(
        &self,
        _ctx: &Context,
        client: &Client,
        url: &UrlEvent,
    ) -> Result<(), ZetaError> {
        let (url, channel) = (url.url(), url.channel());

        match classify_imdb_url(url) {
            Some(Link::Title(id)) => {
                debug!(imdb.title.id = %id, "fetching details for posted title link");

                send_lookup_reply(
                    client,
                    channel,
                    self.client.title(&id),
                    |title| format_title(&title),
                    |error| title_error_reply(&error),
                )
                .await?;
            }
            Some(Link::Person(id)) => {
                debug!(imdb.person.id = %id, "fetching details for posted person link");

                send_lookup_reply(
                    client,
                    channel,
                    self.client.person(&id),
                    |person| format_person(&person),
                    |error| person_error_reply(&error),
                )
                .await?;
            }
            None => {}
        }

        Ok(())
    }
}

/// Formats a failed `!imdb` search.
fn search_error_reply(error: &Error) -> String {
    log_internal_error(error, "imdb search failed");

    format!("{} could not search imdb", format::prefix())
}

/// Formats a failed title lookup.
fn title_error_reply(error: &Error) -> String {
    log_internal_error(error, "imdb title lookup failed");

    let message = match error {
        Error::NotFound => "no results",
        Error::Api(_) | Error::InvalidHeader(_) | Error::GraphQL(_) | Error::UnexpectedResponse => {
            "could not look up the title"
        }
    };

    format!("{} {message}", format::prefix())
}

/// Formats a failed person lookup.
fn person_error_reply(error: &Error) -> String {
    log_internal_error(error, "imdb person lookup failed");

    let message = match error {
        Error::NotFound => "person not found",
        Error::Api(_) | Error::InvalidHeader(_) | Error::GraphQL(_) | Error::UnexpectedResponse => {
            "could not look up the person"
        }
    };

    format!("{} {message}", format::prefix())
}

/// Logs the operator detail of a plugin-owned internal failure.
///
/// [`Error::Api`] failures are already logged once by `zeta-http` when the request or response
/// fails, and [`Error::NotFound`] is an expected miss, so neither is logged here.
fn log_internal_error(error: &Error, message: &str) {
    match error {
        Error::Api(_) | Error::NotFound => {}
        Error::InvalidHeader(_) | Error::GraphQL(_) | Error::UnexpectedResponse => {
            warn!(exception.message = %Chain(error), "{message}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use zeta_http as http;
    use zeta_http::HttpConfig;
    use zeta_http::reqwest;
    use zeta_plugin::irc;
    use zeta_test_support::settings_tests;

    #[test]
    fn cli_definition_is_valid() {
        // clap reports contradictory definitions through asserts; evaluating them here turns a
        // bad definition into a test failure instead of a panic in debug builds.
        Opts::command().debug_assert();
    }

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
    fn parses_queries() {
        let cases = [
            ("multi-word", "peggle nights", "peggle nights"),
            ("quoted", r#""the matrix""#, "the matrix"),
            ("empty", "", ""),
        ];

        for (case, input, expected) in cases {
            let opts: Opts = COMMAND.parse_shell(input).unwrap();

            assert_eq!(opts.query(), expected, "{case}");
        }
    }

    /// Builds an API failure whose body must never reach a reply.
    fn api_error() -> Error {
        Error::Api(http::ApiError::Status {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            body: "secret body".to_owned(),
        })
    }

    /// Builds an invalid-header failure, which only the client constructor can produce.
    fn invalid_header_error() -> Error {
        let error = reqwest::header::HeaderValue::from_str("\n")
            .expect_err("a newline is not a valid header value");

        Error::InvalidHeader(error)
    }

    #[test]
    fn formats_search_failures() {
        let reply = irc::strip_formatting(&search_error_reply(&api_error()));

        assert_eq!(reply, "> IMDb: could not search imdb");
        assert!(!reply.contains("secret body"), "{reply}");
    }

    #[test]
    fn formats_title_lookup_failures() {
        let cases = [
            (Error::NotFound, "no results"),
            (api_error(), "could not look up the title"),
            (
                Error::GraphQL("internal detail".to_string()),
                "could not look up the title",
            ),
            (invalid_header_error(), "could not look up the title"),
            (Error::UnexpectedResponse, "could not look up the title"),
        ];

        for (error, expected) in cases {
            let reply = irc::strip_formatting(&title_error_reply(&error));

            assert_eq!(reply, format!("> IMDb: {expected}"));
            assert!(!reply.contains("internal detail"), "{reply}");
            assert!(!reply.contains("secret body"), "{reply}");
        }
    }

    #[test]
    fn formats_person_lookup_failures() {
        let cases = [
            (Error::NotFound, "person not found"),
            (api_error(), "could not look up the person"),
            (
                Error::GraphQL("internal detail".to_string()),
                "could not look up the person",
            ),
            (invalid_header_error(), "could not look up the person"),
            (Error::UnexpectedResponse, "could not look up the person"),
        ];

        for (error, expected) in cases {
            let reply = irc::strip_formatting(&person_error_reply(&error));

            assert_eq!(reply, format!("> IMDb: {expected}"));
            assert!(!reply.contains("internal detail"), "{reply}");
            assert!(!reply.contains("secret body"), "{reply}");
        }
    }

    #[test]
    fn api_failures_display_a_reply_grade_sentence() {
        let error = api_error();

        assert_eq!(error.to_string(), "the imdb request failed");
        assert!(!error.to_string().contains("secret body"));
        assert!(
            std::error::Error::source(&error).is_some(),
            "the api failure stays in the source chain"
        );
    }

    #[test]
    fn chain_carries_the_api_error_detail() {
        let message = Chain(&api_error()).to_string();

        assert!(message.contains("the imdb request failed"), "{message}");
        assert!(message.contains("500"), "{message}");
    }

    /// Live smoke test against IMDb's GraphQL API, to catch contract drift that the recorded
    /// fixtures cannot. Run with `cargo test -p plugins --all-features -- --ignored imdb::`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs network access"]
    async fn live_api_contract_smoke() {
        let client = GraphQlClient::try_new(&Settings::default(), &HttpConfig::default()).unwrap();

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
