//! Looks up terms in Urban Dictionary.
//!
//! The `.ud <query>` command requests the term from Urban Dictionary's API and replies with
//! the definition and example of the top entry, with runs of whitespace collapsed. The
//! definition and example text is truncated to `max_definition_length` characters (default
//! 400) with an ellipsis suffix; an empty query replies with usage.

use std::fmt::Write;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::debug;

use zeta_http as http;
use zeta_plugin::irc::{labeled_reply, labeled_reply_prefix, reply_lookup};
use zeta_plugin::{Client, CommandEvent, Context, ZetaError, plugin};
use zeta_utils::irc;
use zeta_utils::text::TruncatableExt;

/// The endpoint definitions are looked up against.
const DEFINE_URL: &str = "https://api.urbandictionary.com/v0/define";

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

#[plugin(name = "urban_dictionary", settings = Settings)]
impl UrbanDictionary {
    // The `Plugin` trait requires the constructor to return `Result`, even when initialization
    // cannot fail.
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client = ctx.http().clone();

        Ok(Self {
            client,
            settings: settings.clone(),
        })
    }

    /// Replies to `.ud` with the top Urban Dictionary definition of the query.
    #[command(
        ".ud",
        "Look up the top Urban Dictionary definition",
        event,
        usage = "Usage: .ud\x0f <query>"
    )]
    async fn define(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        reply_lookup(
            client,
            command.channel(),
            self.definitions(command.args()),
            |definitions| {
                definitions.list.first().map_or_else(
                    || labeled_reply("Urban Dictionary", "No results"),
                    |definition| format_definition(definition, self.settings.max_definition_length),
                )
            },
            |error| labeled_reply("Urban Dictionary", error),
        )
        .await?;

        Ok(())
    }
}

/// Formats a definition for IRC, truncating its text to the configured maximum length.
fn format_definition(definition: &Definition, max_length: usize) -> String {
    let word = irc::unstyle_text(&definition.word);
    let definition_text = irc::unstyle_text(&definition.definition);
    let example = irc::unstyle_text(&definition.example);
    let definition_text = definition_text.truncate_with_suffix(max_length, "…");
    let example = example.truncate_with_suffix(max_length, "…");

    let mut message = labeled_reply_prefix("Urban Dictionary");
    let _ = write!(message, "Term:\x0f {word}\x0310");
    let _ = write!(message, " Definition:\x0f {definition_text}\x0310");
    let _ = write!(message, " Example:\x0f {example}");

    message
}

impl UrbanDictionary {
    /// Looks up the given `term` and returns a list of definitions.
    ///
    /// The list of definitions may be empty.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if the request to Urban Dictionary fails or the response cannot be
    /// parsed.
    pub async fn definitions(&self, term: &str) -> Result<Definitions, Error> {
        debug!(urban_dictionary.term = %term, "requesting definitions");
        let params = [("term", term)];
        let request = self.client.get(DEFINE_URL).query(&params);
        let definitions: Definitions = http::get_json(request).await?;
        debug!(
            urban_dictionary.definition.count = %definitions.list.len(),
            "fetched definitions"
        );

        Ok(definitions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use similar_asserts::assert_eq;
    use zeta_test_support::settings_tests;

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
        let expected = format!(
            "{}Term:\x0f word\x0310 Definition:\x0f definition\x0310 Example:\x0f example",
            labeled_reply_prefix("Urban Dictionary")
        );

        assert_eq!(format_definition(&definition, 400), expected);
    }

    #[test]
    fn strips_formatting_from_definitions() {
        let mut definition = test_definition();
        definition.definition = "\x0310Fake\nDefinition\x0f".to_string();

        let message = format_definition(&definition, 400);

        assert!(message.contains("Fake Definition"), "{message}");
        assert!(!message.contains("\x0310Fake"), "{message}");
    }

    #[test]
    fn truncates_long_definitions() {
        let mut definition = test_definition();
        definition.definition = "x".repeat(20);
        definition.example = "y".repeat(20);
        let message = format_definition(&definition, 10);

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
