//! Looks up terms in Urban Dictionary.
//!
//! The `.ud <query>` command requests the term from Urban Dictionary's API and replies with
//! the definition and example of the top entry, with runs of whitespace collapsed. The
//! definition and example text is truncated to `max_definition_length` characters (default
//! 400) with an ellipsis suffix; an empty query replies with usage.

use std::fmt::Display;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::debug;

use crate::{config::HttpConfig, http, plugin::prelude::*, utils::Truncatable, utils::collapse_whitespace};

/// The usage line sent for empty `.ud` queries.
pub const USAGE: &str = "Usage: .ud\x0f <query>";

/// The Urban Dictionary API base URL.
pub const BASE_URL: &str = "https://api.urbandictionary.com";

/// Settings for the `urban_dictionary` plugin, from its `[plugins.urban_dictionary]` configuration
/// section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The maximum length of the definition and example text, in characters.
    pub max_definition_length: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_definition_length: 400,
        }
    }
}

/// The `.ud` command.
const URBAN_DICTIONARY: CommandSpec = CommandSpec::new(
    ".ud",
    "Look up the top Urban Dictionary definition",
);

/// Urban Dictionary plugin.
pub struct UrbanDictionary {
    client: reqwest::Client,
    settings: Settings,
}

/// Errors that can occur during execution.
pub type Error = http::ApiError;

/// List of definitions.
#[derive(Debug, Deserialize)]
pub struct Definitions {
    /// The definitions, most relevant first.
    pub list: Vec<Definition>,
}

/// An Urban Dictionary definition.
#[derive(Debug, Deserialize)]
#[allow(unused)]
pub struct Definition {
    /// The unique id of the definition.
    #[serde(rename = "defid")]
    pub id: u32,
    /// The name of the author who submitted the definition.
    pub author: String,
    /// The literal definition.
    #[allow(clippy::struct_field_names)]
    pub definition: String,
    /// An example usage of the definition.
    pub example: String,
    /// Permalink to the specific definition.
    pub permalink: String,
    /// The word the definition applies to.
    pub word: String,
    /// The number of user thumbs up.
    pub thumbs_up: u32,
    /// The number of user thumbs down.
    pub thumbs_down: u32,
    /// Date and time of when the definition was written.
    #[serde(with = "time::serde::rfc3339")]
    pub written_on: OffsetDateTime,
}

#[async_trait]
impl Plugin<Context> for UrbanDictionary {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        subscriptions.command(URBAN_DICTIONARY);
        Ok(UrbanDictionary::new(&ctx.config.http, settings.clone()))
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let query = command.args();

        let message = if query.is_empty() {
            reply("Urban Dictionary", USAGE)
        } else {
            match self.definitions(query).await {
                Ok(definitions) => definitions.list.first().map_or_else(
                    || reply("Urban Dictionary", "No results"),
                    |definition| {
                        let formatter = DefinitionFormatter {
                            definition,
                            max_length: self.settings.max_definition_length,
                        };

                        reply("Urban Dictionary", formatter.to_string())
                    },
                ),
                Err(err) => reply("Urban Dictionary", err),
            }
        };

        client.send_privmsg(channel, message)?;

        Ok(())
    }
}

/// Formats a definition for IRC, truncating its text to the configured maximum length.
struct DefinitionFormatter<'a> {
    definition: &'a Definition,
    max_length: usize,
}

impl Display for DefinitionFormatter<'_> {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = &self.definition.word;
        let definition = collapse_whitespace(&self.definition.definition);
        let example = collapse_whitespace(&self.definition.example);
        let definition = definition.truncate_with_suffix(self.max_length, "…");
        let example = example.truncate_with_suffix(self.max_length, "…");

        write!(fmt, "Term:\x0f {word}\x0310")?;
        write!(fmt, " Definition:\x0f {definition}\x0310")?;
        write!(fmt, " Example:\x0f {example}")
    }
}

impl UrbanDictionary {
    /// Creates a new plugin instance around a client built for the given HTTP configuration.
    #[must_use]
    pub fn new(config: &HttpConfig, settings: Settings) -> Self {
        let client = http::build_client(config);

        Self { client, settings }
    }

    /// Looks up the given `term` and returns a list of definitions.
    ///
    /// The list of definitions may be empty.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the request to Urban Dictionary fails or the response cannot be
    /// parsed.
    pub async fn definitions(&self, term: &str) -> Result<Definitions, Error> {
        debug!(%term, "requesting definitions");
        let params = [("term", term)];
        let request = self
            .client
            .get(format!("{BASE_URL}/v0/define"))
            .query(&params);
        let definitions: Definitions = http::get_json(request).await?;
        debug!(num_definitions = %definitions.list.len(), "fetched definitions");

        Ok(definitions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a definition for formatting tests.
    fn test_definition() -> Definition {
        Definition {
            id: 1,
            author: "author".to_string(),
            definition: "definition".to_string(),
            example: "example".to_string(),
            permalink: "https://example.com".to_string(),
            word: "word".to_string(),
            thumbs_up: 1,
            thumbs_down: 0,
            written_on: OffsetDateTime::UNIX_EPOCH,
        }
    }

    settings_tests! {
        Settings,
        settings,
        default: {
            assert_eq!(settings.max_definition_length, 400);
        }
        deserialize: {
            "max_definition_length": 100,
        } assert: {
            assert_eq!(settings.max_definition_length, 100);
        }
    }

    #[test]
    fn formats_definitions() {
        let definition = test_definition();
        let formatter = DefinitionFormatter {
            definition: &definition,
            max_length: 400,
        };

        assert_eq!(
            formatter.to_string(),
            "Term:\x0f word\x0310 Definition:\x0f definition\x0310 Example:\x0f example"
        );
    }

    #[test]
    fn truncates_long_definitions() {
        let mut definition = test_definition();
        definition.definition = "x".repeat(20);
        definition.example = "y".repeat(20);
        let formatter = DefinitionFormatter {
            definition: &definition,
            max_length: 10,
        };
        let message = formatter.to_string();

        assert!(
            message.contains(&format!("{}\u{2026}", "x".repeat(10))),
            "{message}"
        );
        assert!(
            message.contains(&format!("{}\u{2026}", "y".repeat(10))),
            "{message}"
        );
    }

    #[test]
    fn decodes_the_api_response() {
        let definitions: Definitions = serde_json::from_str(
            r#"{
                "list": [
                    {
                        "defid": 123,
                        "author": "author",
                        "definition": "the definition",
                        "example": "the example",
                        "permalink": "https://www.urbandictionary.com/define.php?term=word",
                        "word": "word",
                        "thumbs_up": 10,
                        "thumbs_down": 2,
                        "written_on": "2024-01-01T12:00:00.000Z"
                    }
                ]
            }"#,
        )
        .expect("the api response should decode");

        assert_eq!(definitions.list.len(), 1);

        let definition = &definitions.list[0];
        assert_eq!(definition.id, 123);
        assert_eq!(definition.word, "word");
        assert_eq!(definition.thumbs_up, 10);
        // The RFC 3339 timestamp decodes.
        assert_eq!(
            definition.written_on,
            OffsetDateTime::from_unix_timestamp(1_704_110_400).unwrap()
        );
    }
}
