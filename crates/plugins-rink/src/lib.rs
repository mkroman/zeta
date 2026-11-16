//! Evaluates calculations with unit, currency, and cryptocurrency conversions through rink.
//!
//! The `.r <expression>` command evaluates the whole argument as a rink expression and replies
//! with the one-line result as a plain reply — arithmetic, physical unit conversions, and, with
//! live currency data loaded, fiat currency, Bitcoin, and cryptocurrency lookups. The
//! `.c <expression>` command does the same but case-corrects unit tokens rink cannot resolve,
//! so lowercase abbreviations like `10 usd to dkk` evaluate like `10 USD to DKK`. Evaluation
//! errors are reported inline as `> Error: <message>`.
//!
//! Replies are styled like the other plugins: the cyan `> ` marker and scaffolding, with the
//! values — numbers, datetimes, property names, echoed input in errors, category labels, and
//! the unit expressions that follow numbers — reset to the default color to stand out. Rink's
//! output is rendered from its markup spans rather than its plain text form, so the styling can
//! follow the structure of the answer, and control codes are emitted only where the style
//! actually changes — never duplicated, never dangling at the end of the message.
//!
//! A rink context is built once at plugin initialization: the bundled unit definitions, the
//! static currency units, and a small set of extra definitions (CSS lengths, resolutions,
//! angles, and the cryptocurrency subunits) loaded on top. The context is moved into a
//! dedicated actor task when the plugin loads: evaluation calls are serialized through that
//! task's mailbox, and a failed context build aborts plugin initialization.
//!
//! When the `currency` setting is enabled (the default), live currency data is fetched from
//! rink's dataset — fiat rates from the European Central Bank and Bitcoin from blockchain.info
//! — and, when the coinmarketcap plugin is loaded and shares its
//! client, the ranked cryptocurrency table from the CoinMarketCap API; without that feature
//! the cryptocurrency units stay pending. The `crypto` setting turns the cryptocurrency
//! loading off. Both datasets are rebuilt into a fresh context by a background task started
//! when the plugin loads, once per `currency_ttl`; the task asks the actor to swap it in. A
//! query that runs into missing currency data re-evaluates after an inline refresh — mirroring
//! upstream rink's REPL, which fetches on demand in the same situation — but that refresh only
//! runs when the cached dataset is missing, stale, or still lacks a requested cryptocurrency
//! table, so an unresolvable dependency does not fetch once per query; failed fetches are
//! logged and leave the last known rates in place.
//!
//! The implementation is split by concern: this module wires the plugin and holds its state,
//! `commands` evaluates and replies on the command path, `tasks` runs the actor and refresh
//! tasks, `actor` owns the context and serializes evaluations, `currency` plumbs the live
//! dataset, `classify` corrects unit casing, and `render` formats the replies.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use rink_core::Context as RinkContext;
use serde::{Deserialize, Serialize};

mod actor;
mod classify;
mod commands;
mod currency;
mod render;
mod tasks;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

use actor::{Handle, Rewrite};
use currency::{CurrencyData, build_context};

use coinmarketcap::Client as CmcClient;
use zeta_cache::TtlCache;
use zeta_http::reqwest;
use zeta_plugin::{Client, CommandEvent, Context, ZetaError, plugin};

/// Calculator plugin using rink-rs.
pub struct Rink {
    /// The rink engine, moved into the actor task when the plugin loads.
    engine: Option<RinkContext>,
    /// The actor task owning the context and serializing evaluations through its mailbox.
    actor: Option<Handle>,
    /// The fetched live currency data, refreshed at the TTL and used to rebuild the context.
    currency: Arc<TtlCache<CurrencyData>>,
    /// The background task refreshing the currency data, stopped on shutdown.
    refresh_task: Option<tokio::task::JoinHandle<()>>,
    /// The HTTP client used to fetch currency data; `None` when the `currency` setting is
    /// disabled, leaving currency queries to answer with rink's missing-dependencies error.
    client: Option<reqwest::Client>,
    /// The keyed CoinMarketCap client shared by the coinmarketcap plugin, used to fetch the
    /// cryptocurrency listings; `None` when the coinmarketcap plugin is not loaded.
    cmc: Option<Arc<CmcClient>>,
    /// The URL the live currency dataset is fetched from.
    currency_url: String,
    /// Whether to load live cryptocurrency units; effective only when the coinmarketcap plugin
    /// has shared its client.
    crypto_enabled: bool,
}

// Manual implementation: the rink context, actor handle, HTTP client, and shared CoinMarketCap
// client are opaque resources, so only the plugin's own state is shown.
impl fmt::Debug for Rink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rink")
            .field("running", &self.actor.is_some())
            .field(
                "currency_cached",
                &self.currency.with_value(|currency| currency.is_some()),
            )
            .field("crypto_enabled", &self.crypto_enabled)
            .finish_non_exhaustive()
    }
}

/// Settings for the rink plugin, from its `[plugins.rink]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Whether to fetch live currency (fiat and Bitcoin) data.
    pub currency: bool,
    /// How long the fetched currency data stays valid before being refreshed.
    #[serde(with = "humantime_serde")]
    pub currency_ttl: Duration,
    /// The URL the live currency dataset is fetched from.
    pub currency_url: String,
    /// Whether to load live cryptocurrency units through the coinmarketcap plugin.
    ///
    /// Effective only when the plugin has shared its client; without it the cryptocurrency
    /// units stay pending.
    pub crypto: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            currency: true,
            currency_ttl: currency::CURRENCY_TTL,
            currency_url: currency::CURRENCY_URL.to_string(),
            crypto: true,
        }
    }
}

#[plugin(name = "rink", settings = Settings)]
impl Rink {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client = settings.currency.then(|| ctx.http().clone());

        Ok(Self {
            engine: Some(build_context(None, None)?),
            actor: None,
            currency: Arc::new(TtlCache::new(settings.currency_ttl)),
            refresh_task: None,
            client,
            cmc: None,
            currency_url: settings.currency_url.clone(),
            crypto_enabled: settings.crypto,
        })
    }

    #[handler(loaded)]
    async fn loaded(&mut self, ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        // The coinmarketcap plugin publishes its keyed client for other plugins to reuse.
        self.cmc = ctx.shared().get::<CmcClient>();

        self.spawn_actor();
        self.spawn_refresh_task();

        Ok(())
    }

    /// Replies to `.r` with the evaluated calculation.
    #[command(
        ".r",
        "Evaluate a calculation with unit and currency conversions",
        event,
        spec = RINK
    )]
    async fn calculate(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        self.reply_calculation(client, command, Rewrite::Verbatim)
            .await
    }

    /// Replies to `.c` with the evaluated calculation, correcting unit casing.
    #[command(
        ".c",
        "Evaluate a calculation, correcting unit casing",
        event,
        spec = CLASSIFY
    )]
    async fn classify(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        self.reply_calculation(client, command, Rewrite::Classify)
            .await
    }

    #[handler(shutdown)]
    async fn shutdown(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        // The refresh task holds a handle, so it would keep the actor alive after the plugin
        // shuts down. Stopping it and dropping the last handle closes the mailbox, which ends
        // the actor's loop.
        if let Some(task) = self.refresh_task.take() {
            task.abort();
        }

        self.actor = None;

        Ok(())
    }
}
