use crate::plugin::prelude::*;
use tracing::warn;

/// The `.g` command.
const KAGI: Prefix = Prefix::new(".g");

/// The `.gis` command.
const IMAGES: Prefix = Prefix::new(".gis");

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[
    PluginCommand::new(KAGI, "Search with Kagi and link the top result"),
    PluginCommand::new(IMAGES, "Search Kagi Images and link the first result"),
];

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
        command: &Prefix,
        query: &str,
    ) -> Result<(), ZetaError> {
        match *command {
            IMAGES => return self.handle_images(client, channel, query).await,
            KAGI => return self.handle_search(client, channel, query).await,
            _ => {}
        }

        Ok(())
    }
}

impl KagiPlugin {
    /// Handles the `.g` command by linking the top search result for the query.
    async fn handle_search(
        &self,
        client: &Client,
        channel: &str,
        query: &str,
    ) -> Result<(), ZetaError> {
        if query.trim().is_empty() {
            client.send_privmsg(channel, "\x0310> Usage: .g\x0f <query>")?;

            return Ok(());
        }

        match self.client.search(query).await {
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
                warn!(?err, "kagi search failed");
                client.send_privmsg(channel, format!("\x0310> Error: {err}"))?;
            }
        }

        Ok(())
    }

    /// Handles the `.gis` command by linking the first image result for the query.
    async fn handle_images(
        &self,
        client: &Client,
        channel: &str,
        query: &str,
    ) -> Result<(), ZetaError> {
        if query.trim().is_empty() {
            client.send_privmsg(channel, "\x0310> Usage: .gis\x0f <query>")?;

            return Ok(());
        }

        match self.client.images(query).await {
            Ok(results) => {
                if let Some(result) = results.first() {
                    let title = &result.title;
                    let url = &result.image_url;

                    client.send_privmsg(channel, format!("\x0310>\x0f\x02 Kagi:\x02\x0310 {title} - {url}"))?;
                } else {
                    client.send_privmsg(channel, "\x0310> No results")?;
                }
            }
            Err(err) => {
                warn!(?err, "kagi image search failed");
                client.send_privmsg(channel, format!("\x0310> Error: {err}"))?;
            }
        }

        Ok(())
    }
}
