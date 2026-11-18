//! Summarize pages through Kagi Summarize, the way the Kagi browser extension does.
//!
//! The `.tldr <url>` command fetches a summary of the page through the same
//! `kagi.com/mother/summary_labs` endpoint the extension calls, authenticating with the
//! `kagi_session` session token — set in `[plugins.tldr]`, falling back to the
//! `KAGI_SESSION_TOKEN` environment variable shared with the kagi plugin. A missing token fails
//! plugin initialization and the plugin is skipped at startup; the HTTP client, timeout, and
//! user agent come from the shared `[http]` settings.
//!
//! The command takes the extension's options: the summary style (`-t summary|takeaway`), the
//! output language (`-l LANG`, one of the extension's codes), and how much of the summary to
//! send (`-d truncated|full`). Each option resolves per invocation: an explicit flag wins,
//! otherwise the invoking user's preference — declared by the plugin's [`Preferences`] struct
//! and changed with the `.set` command — otherwise the built-in default. Replies open with the
//! `> TLDR:` label; a truncated reply is one budgeted line, a full reply is the whole summary
//! wrapped across several lines.

mod args;
mod client;
mod error;
mod format;
mod model;
mod preferences;

pub use preferences::{Delivery, Language, Preferences, SummaryType};

use std::future::Future;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use zeta_plugin::accounts::PreferenceValue;
use zeta_plugin::irc::{labeled_reply, parse_or_usage, plain_reply, send_reply, send_reply_via};
use zeta_plugin::{
    ArgsMode, Chain, Client, CommandEvent, Context, Registered, ZetaError, plugin, plugin_err,
    resolve_secret,
};

/// How many summaries may be fetched at once.
///
/// The extension produces a summary in seconds; a channel burst of links would otherwise fan
/// out into that many concurrent Kagi requests.
const TLDR_CONCURRENCY: usize = 2;

/// Settings for the tldr plugin, from its `[plugins.tldr]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The Kagi session token (the `kagi_session` cookie value).
    ///
    /// Falls back to the `KAGI_SESSION_TOKEN` environment variable when unset.
    pub session_token: Option<String>,
    /// The `Accept-Language` header sent with summarize requests.
    pub language: String,
    /// The maximum length of a truncated summary reply, in bytes of summary text.
    pub max_summary_length: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            session_token: None,
            language: client::DEFAULT_LANGUAGE.to_owned(),
            max_summary_length: format::DEFAULT_MAX_SUMMARY_LENGTH,
        }
    }
}

/// Kagi Summarize integration.
#[derive(Debug)]
pub struct Tldr {
    /// The summarize client, shaped like the extension's requests.
    client: client::SummaryClient,
    /// The semaphore bounding the concurrent summarize lookups.
    fetches: Arc<Semaphore>,
    /// The maximum length of a truncated summary reply.
    max_summary_length: usize,
}

#[plugin(name = "tldr", settings = Settings, preferences = Preferences)]
impl Tldr {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let token = resolve_secret(settings.session_token.as_deref(), "KAGI_SESSION_TOKEN")?;
        let client = client::SummaryClient::new(ctx.http().clone(), token, &settings.language)
            .map_err(plugin_err)?;

        Ok(Self {
            client,
            fetches: Arc::new(Semaphore::new(TLDR_CONCURRENCY)),
            max_summary_length: settings.max_summary_length,
        })
    }

    /// Replies to `.tldr` with a summary of the page at the given URL.
    #[command(
        ".tldr",
        "Summarize a page with Kagi",
        event,
        usage_hint = "[-t summary|takeaway] [-l LANG] [-d truncated|full] <url>"
    )]
    async fn summarize(
        &self,
        ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let channel = command.channel();

        let Some(opts) = parse_or_usage::<args::Opts>(client, command, ArgsMode::Shell, |line| {
            labeled_reply(format::LABEL, line)
        })?
        else {
            return Ok(());
        };

        let Some(page) = args::page_url(&opts.url) else {
            send_reply(
                client,
                channel,
                labeled_reply(format::LABEL, "give me an http(s) URL to summarize"),
            )?;

            return Ok(());
        };

        // Each option resolves as: explicit flag, then the user's stored preference, then the
        // built-in default. An unidentified sender has no preferences and gets the defaults.
        let identity = command.identity();
        let prefs =
            Preferences::reader(ctx.accounts().preferences(), Self::NAME, identity.as_ref());

        let preferred_summary = preference_or_default(prefs.summary_type()).await;
        let preferred_language = preference_or_default(prefs.language()).await;
        let preferred_delivery = preference_or_default(prefs.delivery()).await;

        let summary_type = opts.summary_type.unwrap_or(preferred_summary);
        let language = opts.language.or(preferred_language);
        let delivery = opts.delivery.unwrap_or(preferred_delivery);

        debug!(
            tldr.summary_type = summary_type.as_str(),
            tldr.language = language.map_or("default", Language::as_str),
            "summarizing a page"
        );

        let http_client = self.client.clone();
        let max_summary_length = self.max_summary_length;

        plugins_shared::spawn_reply_task!(
            Arc::clone(&self.fetches),
            client,
            channel,
            "tldr.summarize",
            &page,
            move |sender, channel| async move {
                let lines = match http_client
                    .summarize(&page, summary_type, language.map(Language::as_str))
                    .await
                {
                    Ok(response) => response.markdown().map_or_else(
                        || vec![plain_reply(error::Error::NoOutput.reply())],
                        |markdown| format::reply_lines(markdown, delivery, max_summary_length),
                    ),
                    Err(err) => {
                        warn!(exception.message = %Chain(&err), "could not summarize the page");

                        vec![plain_reply(err.reply())]
                    }
                };

                for line in lines {
                    if let Err(err) = send_reply_via(&sender, &channel, line) {
                        warn!(exception.message = %err, "could not send the summary");
                    }
                }
            },
        )
        .await;

        Ok(())
    }
}

/// Reads one of the invoking user's preferences, logging a failed read once and falling back to
/// the preference's default.
///
/// A failed read never fails the command: the explicit flag, or the built-in default, still
/// answers, and a user without a stored value simply gets the default.
async fn preference_or_default<T, E>(read: impl Future<Output = Result<T, E>>) -> T
where
    T: Default,
    E: std::error::Error + 'static,
{
    read.await.unwrap_or_else(|error| {
        warn!(
            exception.message = %Chain(&error),
            "could not read a user preference"
        );

        T::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use zeta_test_support::{fixture, settings_tests};

    /// The recorded `summary_type=summary` response from the captured browser session.
    pub(crate) fn summary_fixture() -> &'static str {
        Box::leak(fixture!("summary.json").into_boxed_str())
    }

    /// The recorded `summary_type=takeaway&target_language=DE` response.
    pub(crate) fn takeaway_fixture() -> &'static str {
        Box::leak(fixture!("takeaway.json").into_boxed_str())
    }

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.session_token.is_none());
            assert_eq!(settings.language, client::DEFAULT_LANGUAGE);
            assert_eq!(settings.max_summary_length, format::DEFAULT_MAX_SUMMARY_LENGTH);
            assert!(
                format::DEFAULT_MAX_SUMMARY_LENGTH < zeta_plugin::irc::MAX_LISTING_LENGTH,
                "a truncated reply always fits one IRC line"
            );
        }
        deserialize: {
            "session_token": "secret",
            "language": "da-DK,da;q=0.9",
            "max_summary_length": 200
        } assert: {
            assert_eq!(settings.session_token.as_deref(), Some("secret"));
            assert_eq!(settings.language, "da-DK,da;q=0.9");
            assert_eq!(settings.max_summary_length, 200);
        }
    }

    #[test]
    fn client_debug_never_carries_the_token() {
        let http = zeta_http::reqwest::Client::builder()
            .timeout(Duration::from_secs(1))
            .build()
            .expect("the client builds");
        let configured =
            client::SummaryClient::new(http, "session-secret-token", "en").expect("builds");
        let rendered = format!("{configured:?}");

        assert!(
            !rendered.contains("session-secret-token"),
            "the token never prints: {rendered}"
        );
    }
}
