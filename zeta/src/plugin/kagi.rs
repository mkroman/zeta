use crate::plugin::prelude::*;

/// The `.g` command.
const KAGI: PluginCommand = PluginCommand::new(
    Prefix::new(".g"),
    "Search with Kagi and link the top result",
);

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[KAGI];

/// Kagi search integration.
pub struct KagiPlugin {
    /// Kagi search client.
    client: kagi::Client,
}

#[async_trait]
impl Plugin<Context> for KagiPlugin {
    fn new(_ctx: &Context) -> Result<KagiPlugin, ZetaError> {
        let token = require_env("KAGI_SESSION_TOKEN")?;
        let client = kagi::Client::with_token(token);

        Ok(KagiPlugin { client })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "kagi".into(),
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

        Ok(())
    }
}
