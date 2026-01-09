use std::time::Duration;

use thiserror::Error;

use crate::plugin::prelude::*;

mod client;

/// The duration of a single session. Once this duration has passed, a new session will be created.
pub const KAGI_SESSION_DURATION: Duration = Duration::from_mins(15);

/// Represents a single search result obtained from the search operation.
pub struct SearchResult {
    /// The title of the search result.
    pub title: String,
    /// The URL of the search result.
    pub url: String,
    /// The description.
    #[allow(dead_code)]
    pub description: String,
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("unable to send search request")]
    SearchRequest,
    #[error("could not read response body of search request")]
    SearchRequestBody,
    #[error("could not send nonce request")]
    RequestNonce(#[source] reqwest::Error),
    #[error("could not read nonce response")]
    ReadNonce(#[source] reqwest::Error),
    #[error("could not send session request")]
    RequestSession(#[source] reqwest::Error),
    #[error("response did not include session valid cookies - is the login token valid?")]
    SessionCookies,
    #[error("response did not include a nonce")]
    Nonce,
}

pub struct KagiPlugin {
    /// Kagi search client.
    client: client::Client,
    /// `.g` search command.
    search_command: ZetaCommand,
}

#[async_trait]
impl Plugin for KagiPlugin {
    fn new() -> KagiPlugin {
        let token = std::env::var("KAGI_SESSION_TOKEN")
            .expect("missing KAGI_SESSION_TOKEN environment variable");
        let search_command = ZetaCommand::new(".g");
        let client = client::Client::with_token(token);

        KagiPlugin {
            client,
            search_command,
        }
    }

    fn name() -> Name {
        Name::from("kagi")
    }

    fn author() -> Author {
        Author::from("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version::from("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(query) = self.search_command.parse(user_message)
        {
            let results = self.client.search(query).await;

            match results {
                Ok(results) => {
                    if let Some(result) = results.first() {
                        let title = &result.title;
                        let url = &result.url;

                        client.send_privmsg(channel, format!("\x0310> {title} - {url}"))?;
                    } else {
                        client.send_privmsg(channel, "\x0310> No results")?;
                    }
                }
                Err(err) => {
                    client.send_privmsg(channel, format!("Error: {err}"))?;
                }
            }
        }

        Ok(())
    }
}
