use std::fmt::Display;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::debug;

use crate::{
    config::HttpConfig,
    http,
    plugin::prelude::*,
    utils::Truncatable,
};

pub const USAGE: &str = "Usage: .ud\x0f <query>";
pub const BASE_URL: &str = "https://api.urbandictionary.com";

/// Settings for the urban_dictionary plugin, from its `[plugins.urban_dictionary]` configuration
/// section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The maximum length of the definition and example text, in characters.
    #[serde(default = "default_max_definition_length")]
    pub max_definition_length: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_definition_length: default_max_definition_length(),
        }
    }
}

/// Returns the default maximum length of the definition and example text.
const fn default_max_definition_length() -> usize {
    400
}

/// The `.ud` command.
const URBAN_DICTIONARY: PluginCommand = PluginCommand::new(
    Prefix::new(".ud"),
    "Look up the top Urban Dictionary definition",
);

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[URBAN_DICTIONARY];

/// Urban Dictionary plugin.
pub struct UrbanDictionary {
    client: reqwest::Client,
    settings: Settings,
}

/// Errors that can occur during execution.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("request error: {0}")]
    Request(#[source] reqwest::Error),
    #[error("unable to parse list of definitions: {0}")]
    ParseDefinitions(#[source] reqwest::Error),
}

/// List of definitions.
#[derive(Debug, Deserialize)]
pub struct Definitions {
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
    fn new(ctx: &Context) -> Result<Self, ZetaError> {
        Ok(UrbanDictionary::new(
            &ctx.config.http,
            ctx.config.plugins.urban_dictionary.settings.clone(),
        ))
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "urban_dictionary".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn commands(&self) -> &'static [PluginCommand] {
        COMMANDS
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        channel: &str,
        _command: &Prefix,
        query: &str,
    ) -> Result<(), ZetaError> {
        if query.is_empty() {
            client.send_privmsg(channel, formatted(USAGE))?;
            return Ok(());
        }

        match self.definitions(query).await {
            Ok(definitions) => {
                if let Some(definition) = definitions.list.first() {
                    let formatter = DefinitionFormatter {
                        definition,
                        max_length: self.settings.max_definition_length,
                    };
                    let s = formatted(&formatter.to_string());

                    client.send_privmsg(channel, s)?;
                } else {
                    client.send_privmsg(channel, formatted("No results"))?;
                }
            }
            Err(err) => {
                client.send_privmsg(channel, formatted(&format!("Error: {err}")))?;
            }
        }

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
        let definition = presentable(&self.definition.definition);
        let example = presentable(&self.definition.example);
        let definition = definition.truncate_with_suffix(self.max_length, "…");
        let example = example.truncate_with_suffix(self.max_length, "…");

        write!(fmt, "Term:\x0f {word}\x0310")?;
        write!(fmt, " Definition:\x0f {definition}\x0310")?;
        write!(fmt, " Example:\x0f {example}")
    }
}

/// Renders the given input string in an IRC-presentable way by removing carriage returns,
/// replacing newlines with spaces and trimming leading and trailing whitespace.
fn presentable(s: &str) -> String {
    s.trim().replace('\r', "").replace('\n', " ")
}

fn formatted(s: &str) -> String {
    format!("\x0310>\x03\x02 Urban Dictionary:\x02\x0310 {s}")
}

impl UrbanDictionary {
    pub fn new(config: &HttpConfig, settings: Settings) -> Self {
        let client = http::build_client(config);

        Self { client, settings }
    }

    /// Looks up the given `term` and returns a list of definitions.
    ///
    /// The list of definitions may be empty.
    ///
    /// # Returns
    ///
    /// On success, returns [`Ok(Definitions)`]
    ///
    pub async fn definitions(&self, term: &str) -> Result<Definitions, Error> {
        debug!(%term, "requesting definitions");
        let params = [("term", term)];
        let request = self
            .client
            .get(format!("{BASE_URL}/v0/define"))
            .query(&params);
        let response = request.send().await.map_err(Error::Request)?;

        match response.error_for_status() {
            Ok(response) => {
                let definitions: Definitions =
                    response.json().await.map_err(Error::ParseDefinitions)?;
                debug!(num_definitions = %definitions.list.len(), "fetched definitions");

                Ok(definitions)
            }
            Err(err) => Err(Error::Request(err)),
        }
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

    #[test]
    fn default_settings() {
        assert_eq!(Settings::default().max_definition_length, 400);
    }

    #[test]
    fn settings_deserialize() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "max_definition_length": 100,
        }))
        .expect("could not deserialize settings");

        assert_eq!(settings.max_definition_length, 100);
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
}
