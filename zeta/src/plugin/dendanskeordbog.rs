use std::fmt::{self, Display};

use dendanskeordbog::DictionaryDocument;

use crate::{command::Command as ZetaCommand, http, plugin::prelude::*};

pub struct DenDanskeOrdbog {
    client: dendanskeordbog::Client,
    command: ZetaCommand,
}

struct MessageFormatter(DictionaryDocument);

impl Display for MessageFormatter {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(entry) = self.0.entries.first() {
            let word = &entry.head.keyword;
            write!(fmt, "\x0310>\x0f\x02 DDO:\x02\x0310 {word}")?;

            if let Some(phonetic) = &entry.phonetic {
                write!(fmt, " {phonetic}")?;
            }

            let pos = &entry.pos;
            write!(fmt, " (\x0f{pos}\x0310)")?;

            if let Some(inflection) = &entry.morphology {
                write!(fmt, " Bøjning:\x0f {inflection}\x0310")?;
            }

            if let Some(etymology) = &entry.etymology {
                write!(fmt, " Oprindelse:\x0f {etymology}\x0310")?;
            }

            if let Some(definition) = &entry.definitions.first() {
                let description = &definition.description;
                write!(fmt, " Definition:\x0f {description}\x0310")?;

                if let Some(example) = &definition.examples.first() {
                    write!(fmt, " Eksempel:\x0f {example}\x0310")?;
                }
            }
        } else {
            write!(fmt, "\x0310> No results")?;
        }

        Ok(())
    }
}

#[async_trait]
impl Plugin for DenDanskeOrdbog {
    fn new() -> DenDanskeOrdbog {
        DenDanskeOrdbog::new()
    }

    fn name() -> Name {
        Name("dendanskeordbog")
    }

    fn author() -> Author {
        Author("Mikkel Kroman <mk@maero.dk>")
    }

    fn version() -> Version {
        Version("0.1")
    }

    async fn handle_message(&self, message: &Message, client: &Client) -> Result<(), ZetaError> {
        if let Command::PRIVMSG(ref channel, ref user_message) = message.command
            && let Some(args) = self.command.parse(user_message)
        {
            if args.is_empty() {
                client.send_privmsg(channel, "\x0310> Usage: .ddo\x0f <query>")?;
            } else {
                match self.client.query(args).await {
                    Ok(document) => {
                        client.send_privmsg(channel, MessageFormatter(document).to_string())?;
                    }
                    Err(err) => {
                        client.send_privmsg(channel, format!("\x0310> Error: {err}"))?;
                    }
                }
            }
        }

        Ok(())
    }
}

impl DenDanskeOrdbog {
    pub fn new() -> DenDanskeOrdbog {
        let http_client = http::build_client();
        let client = dendanskeordbog::Client::with_client(http_client);
        let command = ZetaCommand::new(".ddo");

        DenDanskeOrdbog { client, command }
    }
}
