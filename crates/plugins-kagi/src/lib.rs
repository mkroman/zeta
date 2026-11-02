//! Search the web and images through Kagi.
//!
//! The `.g <query>` command posts the title and URL of the top web result, and `.gis <query>`
//! posts the first Kagi Images result (with its image URL), both as a reply prefixed with the
//! Kagi name. No results and search errors are sent as a plain reply.
//!
//! Kagi is a cookie-authenticated service: the client holds a session token — set in
//! `[plugins.kagi]`, falling back to the `KAGI_SESSION_TOKEN` environment variable — and
//! refreshes the session after `session_duration` (with an `Accept-Language` header taken from
//! the `language` setting). A missing token fails plugin initialization and the plugin is
//! skipped at startup; the HTTP timeout and user agent come from the shared `[http]` settings.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

use zeta_plugin::prelude::*;

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
pub struct Kagi {
    /// Kagi search client.
    client: kagi::Client,
}

#[plugin(name = "kagi", settings = Settings)]
impl Kagi {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let token = resolve_secret(settings.session_token.as_deref(), "KAGI_SESSION_TOKEN")?;
        let options = kagi::ClientOptions {
            timeout: ctx.config.http.timeout,
            user_agent: ctx.config.http.user_agent.clone(),
            session_duration: settings.session_duration,
            language: settings.language.clone(),
        };
        let client = kagi::Client::with_token_and_options(token, options).map_err(plugin_err)?;

        Ok(Self { client })
    }

    /// Replies to `.g` with the top search result for the query.
    #[command(
        ".g",
        "Search with Kagi and link the top result",
        event,
        usage = "Usage: .g\x0f <query>"
    )]
    async fn search(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        reply_lookup(
            client,
            command.channel(),
            self.client.search(command.args()),
            |results| {
                results.first().map_or_else(
                    || plain_reply("No results"),
                    |result| {
                        labeled_reply(
                            "Kagi",
                            format!("{} - {}", unstyle_text(&result.title), result.url),
                        )
                    },
                )
            },
            |err| {
                warn!(exception.message = %err, "kagi search failed");
                plain_reply(err)
            },
        )
        .await?;

        Ok(())
    }

    /// Replies to `.gis` with the first image result for the query.
    #[command(
        ".gis",
        "Search Kagi Images and link the first result",
        event,
        usage = "Usage: .gis\x0f <query>"
    )]
    async fn search_images(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        reply_lookup(
            client,
            command.channel(),
            self.client.images(command.args()),
            |results| {
                results.first().map_or_else(
                    || plain_reply("No results"),
                    |result| {
                        labeled_reply(
                            "Kagi",
                            format!("{} - {}", unstyle_text(&result.title), result.image_url),
                        )
                    },
                )
            },
            |err| {
                warn!(exception.message = %err, "kagi image search failed");
                plain_reply(err)
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
