use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::plugin::prelude::*;

/// The `.g` command.
const KAGI: Prefix = Prefix::new(".g");

/// The `.gis` command.
const IMAGES: Prefix = Prefix::new(".gis");

/// Settings for the kagi plugin, from its `[plugins.kagi]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// The Kagi session token (the `kagi_session` cookie value).
    ///
    /// Falls back to the `KAGI_SESSION_TOKEN` environment variable when unset.
    #[serde(default)]
    pub session_token: Option<String>,
    /// How long a search session stays valid before it is refreshed.
    #[serde(default = "default_session_duration", with = "humantime_serde")]
    pub session_duration: Duration,
    /// The `Accept-Language` header sent with requests.
    #[serde(default = "default_language")]
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            session_token: None,
            session_duration: default_session_duration(),
            language: default_language(),
        }
    }
}

/// Returns the default session duration.
const fn default_session_duration() -> Duration {
    kagi::SESSION_DURATION
}

/// Returns the default `Accept-Language` header.
fn default_language() -> String {
    kagi::LANGUAGE.to_string()
}

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
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings) -> Result<KagiPlugin, ZetaError> {
        let token = resolve_secret(settings.session_token.as_deref(), "KAGI_SESSION_TOKEN")?;
        let options = kagi::ClientOptions {
            timeout: ctx.config.http.timeout,
            user_agent: ctx.config.http.user_agent.clone(),
            session_duration: settings.session_duration,
            language: settings.language.clone(),
        };
        let client = kagi::Client::with_token_and_options(token, options).map_err(plugin_err)?;

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
            client.send_privmsg(channel, notice("Usage: .g\x0f <query>"))?;

            return Ok(());
        }

        match self.client.search(query).await {
            Ok(results) => {
                if let Some(result) = results.first() {
                    let title = &result.title;
                    let url = &result.url;

                    client.send_privmsg(channel, notice(format!("{title} - {url}")))?;
                } else {
                    client.send_privmsg(channel, notice("No results"))?;
                }
            }
            Err(err) => {
                warn!(?err, "kagi search failed");
                client.send_privmsg(channel, notice(format!("Error: {err}")))?;
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
            client.send_privmsg(channel, notice("Usage: .gis\x0f <query>"))?;

            return Ok(());
        }

        match self.client.images(query).await {
            Ok(results) => {
                if let Some(result) = results.first() {
                    let title = &result.title;
                    let url = &result.image_url;

                    client.send_privmsg(channel, reply("Kagi", format!("{title} - {url}")))?;
                } else {
                    client.send_privmsg(channel, notice("No results"))?;
                }
            }
            Err(err) => {
                warn!(?err, "kagi image search failed");
                client.send_privmsg(channel, notice(format!("Error: {err}")))?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings() {
        let settings = Settings::default();

        assert!(settings.session_token.is_none());
        assert_eq!(settings.session_duration, kagi::SESSION_DURATION);
        assert_eq!(settings.language, kagi::LANGUAGE);
    }

    #[test]
    fn settings_deserialize() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "session_token": "secret",
            "session_duration": "1h",
            "language": "da-DK,da;q=0.9",
        }))
        .expect("could not deserialize settings");

        assert_eq!(settings.session_token.as_deref(), Some("secret"));
        assert_eq!(settings.session_duration, Duration::from_hours(1));
        assert_eq!(settings.language, "da-DK,da;q=0.9");
    }
}
