use std::fmt::Display;

use serde::Deserialize;
use time::OffsetDateTime;
use tracing::debug;

use crate::{http, plugin::prelude::*};

pub const USAGE: &str = "Usage: .ud\x0f <query>";
pub const BASE_URL: &str = "https://api.urbandictionary.com";

/// Urban Dictionary plugin.
pub struct UrbanDictionary {
    client: reqwest::Client,
    command: ZetaCommand,
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
impl Plugin for UrbanDictionary {
    fn new() -> Self {
        UrbanDictionary::new()
    }

    fn name() -> Name {
        Name::from("urban_dictionary")
    }

    fn author() -> Author {
        Author::from("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version::from("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command {
            match self.command.parse(user_message) {
                Some("") => {
                    client.send_privmsg(channel, formatted(USAGE))?;
                }
                Some(query) => match self.definitions(query).await {
                    Ok(definitions) => {
                        if let Some(definition) = definitions.list.first() {
                            let s = formatted(&format!("{definition}"));
                            client.send_privmsg(channel, s)?;
                        } else {
                            client.send_privmsg(channel, formatted("No results"))?;
                        }
                    }
                    Err(err) => {
                        client.send_privmsg(channel, formatted(&format!("Error: {err}")))?;
                    }
                },
                None => {}
            }
        }

        Ok(())
    }
}

impl Display for Definition {
    fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let word = &self.word;
        let definition = presentable(&self.definition);
        let example = presentable(&self.example);

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
    pub fn new() -> Self {
        let client = http::build_client();
        let command = ZetaCommand::new(".ud");

        UrbanDictionary { client, command }
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
