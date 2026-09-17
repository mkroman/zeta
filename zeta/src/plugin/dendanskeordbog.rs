use std::fmt::{self, Display};

use dendanskeordbog::DictionaryDocument;
use serde::{Deserialize, Serialize};

use crate::{config::HttpConfig, http, plugin::prelude::*};

/// The `.ddo` command.
const DDO: PluginCommand =
    PluginCommand::new(Prefix::new(".ddo"), "Look up a word in Den Danske Ordbog");

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[DDO];

/// Settings for the dendanskeordbog plugin, from its `[plugins.dendanskeordbog]` configuration
/// section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Whether to show the morphology (inflection) of the entry.
    #[serde(default = "default_true")]
    pub show_morphology: bool,
    /// Whether to show the etymology (origin) of the entry.
    #[serde(default = "default_true")]
    pub show_etymology: bool,
    /// Whether to show an example sentence for the definition.
    #[serde(default = "default_true")]
    pub show_examples: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_morphology: default_true(),
            show_etymology: default_true(),
            show_examples: default_true(),
        }
    }
}

/// Returns the default value for the boolean display settings.
const fn default_true() -> bool {
    true
}

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

#[async_trait]
impl Plugin<Context> for DenDanskeOrdbog {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings) -> Result<DenDanskeOrdbog, ZetaError> {
        Ok(DenDanskeOrdbog::new(&ctx.config.http, settings.clone()))
    }

    const COMMANDS: &'static [PluginCommand] = COMMANDS;

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        channel: &str,
        _command: &Prefix,
        args: &str,
    ) -> Result<(), ZetaError> {
        if args.is_empty() {
            client.send_privmsg(channel, notice("Usage: .ddo\x0f <query>"))?;
        } else {
            match self.client.query(args).await {
                Ok(document) => {
                    let formatter = MessageFormatter {
                        document: &document,
                        settings: &self.settings,
                    };

                    client.send_privmsg(channel, formatter.to_string())?;
                }
                Err(err) => {
                    client.send_privmsg(channel, notice(format!("Error: {err}")))?;
                }
            }
        }

        Ok(())
    }
}

impl DenDanskeOrdbog {
    pub fn new(config: &HttpConfig, settings: Settings) -> DenDanskeOrdbog {
        let http_client = http::build_client(config);
        let client = dendanskeordbog::Client::with_client(http_client);

        DenDanskeOrdbog { client, settings }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
