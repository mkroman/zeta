//! Looks up Danish words in Den Danske Ordbog.
//!
//! The `.ddo <word>` command queries the dictionary's web service through the `dendanskeordbog`
//! crate and replies with the phonetics and part of speech of the first matching entry, its
//! inflection (Bøjning), origin (Oprindelse), first definition, and an example sentence
//! (Eksempel). No results and lookup errors are sent as a plain reply.
//!
//! The inflection, etymology, and example are optional; each is shown by default and can be
//! turned off in `[plugins.dendanskeordbog]`.

use std::fmt::Write;

use dendanskeordbog::DictionaryDocument;
use serde::{Deserialize, Serialize};
use tracing::warn;

use zeta_plugin::irc::{labeled_reply_prefix, plain_reply, reply_lookup, unstyle_text};
use zeta_plugin::{Client, CommandEvent, Context, ZetaError, plugin};

/// Settings for the dendanskeordbog plugin, from its `[plugins.dendanskeordbog]` configuration
/// section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Whether to show the morphology (inflection) of the entry.
    pub show_morphology: bool,
    /// Whether to show the etymology (origin) of the entry.
    pub show_etymology: bool,
    /// Whether to show an example sentence for the definition.
    pub show_examples: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_morphology: true,
            show_etymology: true,
            show_examples: true,
        }
    }
}

/// The Den Danske Ordbog plugin: looks up Danish words on behalf of `.ddo` commands.
pub struct DenDanskeOrdbog {
    client: dendanskeordbog::Client,
    settings: Settings,
}

fn format_message(document: &DictionaryDocument, settings: &Settings) -> String {
    let Some(entry) = document.entries.first() else {
        return plain_reply("No results");
    };

    let mut message = labeled_reply_prefix("DDO");

    if let Some(phonetic) = &entry.phonetic {
        let _ = write!(message, " {}", unstyle_text(phonetic));
    }

    let pos = unstyle_text(&entry.pos);
    let _ = write!(message, " (\x0f{pos}\x0310)");

    if settings.show_morphology
        && let Some(inflection) = &entry.morphology
    {
        let _ = write!(message, " Bøjning:\x0f {}\x0310", unstyle_text(inflection));
    }

    if settings.show_etymology
        && let Some(etymology) = &entry.etymology
    {
        let _ = write!(
            message,
            " Oprindelse:\x0f {}\x0310",
            unstyle_text(etymology)
        );
    }

    if let Some(definition) = &entry.definitions.first() {
        let description = unstyle_text(&definition.description);
        let _ = write!(message, " Definition:\x0f {description}\x0310");

        if settings.show_examples
            && let Some(example) = &definition.examples.first()
        {
            let _ = write!(message, " Eksempel:\x0f {}\x0310", unstyle_text(example));
        }
    }

    message
}

#[plugin(name = "dendanskeordbog", settings = Settings)]
impl DenDanskeOrdbog {
    // The `Plugin` trait requires the constructor to return `Result`, even when initialization
    // cannot fail.
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let http_client = ctx.http().clone();
        let client = dendanskeordbog::Client::with_client(http_client);

        Ok(Self {
            client,
            settings: settings.clone(),
        })
    }

    /// Replies to `.ddo` with the dictionary entry of the queried word.
    #[command(
        ".ddo",
        "Look up a word in Den Danske Ordbog",
        event,
        usage = "Usage: .ddo\x0f <query>"
    )]
    async fn lookup(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        reply_lookup(
            client,
            command.channel(),
            self.client.query(command.args()),
            |document| format_message(&document, &self.settings),
            |error| {
                warn!(exception.message = %error, "ddo lookup failed");
                plain_reply(error)
            },
        )
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.show_morphology);
            assert!(settings.show_etymology);
            assert!(settings.show_examples);
        }
        deserialize: {
            "show_morphology": false,
            "show_etymology": false,
            "show_examples": false,
        } assert: {
            assert!(!settings.show_morphology);
            assert!(!settings.show_etymology);
            assert!(!settings.show_examples);
        }
    }
}
