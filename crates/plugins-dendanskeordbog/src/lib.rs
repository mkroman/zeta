//! Looks up Danish words in Den Danske Ordbog.
//!
//! The `.ddo <word>` command queries the dictionary's web service through the `dendanskeordbog`
//! crate and replies with the phonetics and part of speech of the first matching entry, its
//! inflection (Bøjning), origin (Oprindelse), first definition, and an example sentence
//! (Eksempel). No results and lookup errors are sent as a notice.
//!
//! The inflection, etymology, and example are optional; each is shown by default and can be
//! turned off in `[plugins.dendanskeordbog]`.

use std::fmt::{self, Display};

use dendanskeordbog::DictionaryDocument;
use serde::{Deserialize, Serialize};

use zeta_http as http;
use zeta_plugin::prelude::*;

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

struct MessageFormatter<'a> {
    document: &'a DictionaryDocument,
    settings: &'a Settings,
}

impl Display for MessageFormatter<'_> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(entry) = self.document.entries.first() {
            write!(fmt, "{}", reply_prefix("DDO"))?;

            if let Some(phonetic) = &entry.phonetic {
                write!(fmt, " {phonetic}")?;
            }

            let pos = &entry.pos;
            write!(fmt, " (\x0f{pos}\x0310)")?;

            if self.settings.show_morphology
                && let Some(inflection) = &entry.morphology
            {
                write!(fmt, " Bøjning:\x0f {inflection}\x0310")?;
            }

            if self.settings.show_etymology
                && let Some(etymology) = &entry.etymology
            {
                write!(fmt, " Oprindelse:\x0f {etymology}\x0310")?;
            }

            if let Some(definition) = &entry.definitions.first() {
                let description = &definition.description;
                write!(fmt, " Definition:\x0f {description}\x0310")?;

                if self.settings.show_examples
                    && let Some(example) = &definition.examples.first()
                {
                    write!(fmt, " Eksempel:\x0f {example}\x0310")?;
                }
            }
        } else {
            write!(fmt, "{}", notice("No results"))?;
        }

        Ok(())
    }
}

#[plugin(name = "dendanskeordbog", settings = Settings)]
impl DenDanskeOrdbog {
    // The `Plugin` trait requires the constructor to return `Result`, even when initialization
    // cannot fail.
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let http_client = http::build_client(&ctx.config.http);
        let client = dendanskeordbog::Client::with_client(http_client);

        Ok(Self {
            client,
            settings: settings.clone(),
        })
    }

    /// Replies to `.ddo` with the dictionary entry of the queried word.
    #[command(".ddo", "Look up a word in Den Danske Ordbog", event)]
    async fn lookup(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();
        let args = command.args();

        let message = if args.is_empty() {
            notice("Usage: .ddo\x0f <query>")
        } else {
            match self.client.query(args).await {
                Ok(document) => MessageFormatter {
                    document: &document,
                    settings: &self.settings,
                }
                .to_string(),
                Err(err) => notice(err),
            }
        };

        client.send_privmsg(channel, message)?;

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
