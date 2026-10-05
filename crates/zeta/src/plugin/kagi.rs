//! Search the web and images through Kagi.
//!
//! The `.g <query>` command posts the title and URL of the top web result, and `.gis <query>`
//! posts the first Kagi Images result (with its image URL), both as a reply prefixed with the
//! Kagi name. No results and search errors are sent as a notice.
//!
//! Kagi is a cookie-authenticated service: the client holds a session token — set in
//! `[plugins.kagi]`, falling back to the `KAGI_SESSION_TOKEN` environment variable — and
//! refreshes the session after `session_duration` (with an `Accept-Language` header taken from
//! the `language` setting). A missing token fails plugin initialization and the plugin is
//! skipped at startup; the HTTP timeout and user agent come from the shared `[http]` settings.

use std::future::Future;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::plugin::prelude::*;

/// The `.g` command.
const KAGI: CommandSpec = CommandSpec::new(".g", "Search with Kagi and link the top result");

/// The `.gis` command.
const IMAGES: CommandSpec = CommandSpec::new(".gis", "Search Kagi Images and link the first result");

/// Settings for the kagi plugin, from its `[plugins.kagi]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The Kagi session token (the `kagi_session` cookie value).
    ///
    /// Falls back to the `KAGI_SESSION_TOKEN` environment variable when unset.
    pub session_token: Option<String>,
    /// How long a search session stays valid before it is refreshed.
    #[serde(with = "humantime_serde")]
    pub session_duration: Duration,
    /// The `Accept-Language` header sent with requests.
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            session_token: None,
            session_duration: kagi::SESSION_DURATION,
            language: kagi::LANGUAGE.to_string(),
        }
    }
}

/// Kagi search integration.
pub struct KagiPlugin {
    /// Kagi search client.
    client: kagi::Client,
}

#[async_trait]
impl Plugin<Context> for KagiPlugin {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<KagiPlugin, ZetaError> {
        let token = resolve_secret(settings.session_token.as_deref(), "KAGI_SESSION_TOKEN")?;
        let options = kagi::ClientOptions {
            timeout: ctx.config.http.timeout,
            user_agent: ctx.config.http.user_agent.clone(),
            session_duration: settings.session_duration,
            language: settings.language.clone(),
        };
        let client = kagi::Client::with_token_and_options(token, options).map_err(plugin_err)?;

        subscriptions.command(KAGI).command(IMAGES);

        Ok(KagiPlugin { client })
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        match command.spec {
            KAGI => {
                let search = |query: String| async move {
                    self.client.search(&query).await.map(|results| {
                        results
                            .into_iter()
                            .map(|result| (result.title, result.url))
                            .collect()
                    })
                };

                return self
                    .handle_lookup(
                        client,
                        command.channel(),
                        command.args(),
                        "Usage: .g\x0f <query>",
                        search,
                    )
                    .await;
            }
            IMAGES => {
                let search = |query: String| async move {
                    self.client.images(&query).await.map(|results| {
                        results
                            .into_iter()
                            .map(|result| (result.title, result.image_url))
                            .collect()
                    })
                };

                return self
                    .handle_lookup(
                        client,
                        command.channel(),
                        command.args(),
                        "Usage: .gis\x0f <query>",
                        search,
                    )
                    .await;
            }
            _ => {}
        }

        Ok(())
    }
}

impl KagiPlugin {
    /// Handles the `.g` and `.gis` commands by linking the top result for the query.
    ///
    /// `search` runs the query and yields the results as `(title, url)` pairs; only the top
    /// result is linked.
    async fn handle_lookup<F, Fut>(
        &self,
        client: &Client,
        channel: &str,
        query: &str,
        usage: &str,
        search: F,
    ) -> Result<(), ZetaError>
    where
        F: FnOnce(String) -> Fut,
        Fut: Future<Output = Result<Vec<(String, String)>, kagi::Error>>,
    {
        let message = if query.trim().is_empty() {
            notice(usage)
        } else {
            match search(query.to_string()).await {
                Ok(results) => results.first().map_or_else(
                    || notice("No results"),
                    |(title, url)| reply("Kagi", format!("{title} - {url}")),
                ),
                Err(err) => {
                    warn!(?err, "kagi search failed");
                    notice(err)
                }
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
            assert!(settings.session_token.is_none());
            assert_eq!(settings.session_duration, kagi::SESSION_DURATION);
            assert_eq!(settings.language, kagi::LANGUAGE);
        }
        deserialize: {
            "session_token": "secret",
            "session_duration": "1h",
            "language": "da-DK,da;q=0.9",
        } assert: {
            assert_eq!(settings.session_token.as_deref(), Some("secret"));
            assert_eq!(settings.session_duration, Duration::from_hours(1));
            assert_eq!(settings.language, "da-DK,da;q=0.9");
        }
    }
}
