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
//! logged and leave the last known rates in place. The dataset plumbing lives in the `currency`
//! submodule.

use std::sync::Arc;
use std::time::Duration;

use rink_core::Context as RinkContext;
use serde::{Deserialize, Serialize};
use tokio::time::MissedTickBehavior;
use tracing::{Instrument, debug, warn};

mod actor;
mod classify;
mod currency;
mod render;
#[cfg(test)]
mod test_support;
use actor::{EvalOutcome, Handle};
use classify::classify;
use currency::{CurrencyData, build_context, refresh_data};
use render::{error_message, render_reply};

use coinmarketcap::Client as CmcClient;
use zeta_cache::TtlCache;
use zeta_http::RequestError;
use zeta_plugin::prelude::*;

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
        let client = if settings.currency {
            Some(
                zeta_http::builder(&ctx.config.http)
                    .build()
                    .map_err(|error| plugin_err(RequestError::from(error)))?,
            )
        } else {
            None
        };

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
        self.cmc = ctx.shared.get::<CmcClient>();

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
        let message = self.eval_and_format(command.args(), false).await?;

        client.send_privmsg(command.channel(), message)?;

        Ok(())
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
        let message = self.eval_and_format(command.args(), true).await?;

        client.send_privmsg(command.channel(), message)?;

        Ok(())
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

impl Rink {
    /// Moves the engine into the actor task that serializes evaluations.
    fn spawn_actor(&mut self) {
        let engine = self.engine.take().expect("the rink context is built once");

        self.actor = Some(Handle::spawn(engine));
    }

    /// Starts the background task that refreshes the currency data at the TTL.
    ///
    /// The task fetches on its first tick and again whenever the cached data expires, asking the
    /// actor to swap in a context rebuilt with the new rates. Requires the `currency` setting to
    /// have created an HTTP client and the actor to be running.
    fn spawn_refresh_task(&mut self) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(actor) = self.actor.clone() else {
            return;
        };

        let (currency, url) = (Arc::clone(&self.currency), self.currency_url.clone());
        let (cmc, crypto_enabled) = (self.cmc.clone(), self.crypto_enabled);

        self.refresh_task = Some(tokio::spawn(
            async move {
                debug!("starting currency refresh task");

                let mut interval = tokio::time::interval(currency::CURRENCY_CHECK_INTERVAL);
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

                loop {
                    interval.tick().await;

                    let refreshed = currency
                        .refresh(|| {
                            refresh_data(
                                &client,
                                &currency,
                                cmc.as_deref(),
                                crypto_enabled,
                                &url,
                                &actor,
                            )
                        })
                        .await;

                    if let Err(error) = refreshed {
                        warn!(%error, "could not refresh currency data");
                    }
                }
            }
            .instrument(tracing::info_span!("currency_refresh")),
        ));
    }

    /// Evaluates `line` and renders the result or error as a one-line IRC reply.
    ///
    /// When `classify` is set, unit tokens rink cannot resolve are case-corrected against the
    /// registry first, so `.c 10 usd to dkk` evaluates like `10 USD to DKK`.
    ///
    /// A query that runs into missing currency data triggers an inline refresh and a
    /// re-evaluation, mirroring upstream rink's REPL, which fetches on demand in the same
    /// situation. The refresh is only attempted when it can actually change the outcome —
    /// the cached dataset is missing, stale, or lacks a requested cryptocurrency table — so
    /// a dependency that stays unresolvable does not fetch on every query. A failed refresh
    /// is logged and the missing-dependencies error is answered instead.
    ///
    /// # Errors
    ///
    /// Returns a [`ZetaError`] when the rink actor has stopped.
    async fn eval_and_format(&self, line: &str, classify: bool) -> Result<String, ZetaError> {
        let actor = self
            .actor
            .as_ref()
            .ok_or_else(|| plugin_err(std::io::Error::other("the rink actor is not running")))?;

        let retry_line = match actor.eval(line.to_owned(), classify).await? {
            EvalOutcome::Answer(text) => return Ok(text),
            EvalOutcome::NeedsCurrency { error_reply, .. } if self.client.is_none() => {
                return Ok(error_reply);
            }
            EvalOutcome::NeedsCurrency { retry_line, .. } => retry_line,
        };

        // Currency data is missing: refresh what can still change the outcome, then
        // re-evaluate the same classified line, mirroring upstream rink's REPL, which fetches
        // on demand in the same situation.
        self.ensure_currency_cached().await;

        match actor.eval(retry_line, false).await? {
            EvalOutcome::Answer(text) => Ok(text),
            EvalOutcome::NeedsCurrency { error_reply, .. } => Ok(error_reply),
        }
    }

    /// Fetches and loads currency data when a refetch can still change the outcome.
    ///
    /// Used when a query hit missing dependencies. Like the coinmarketcap plugin's
    /// `ensure_coins_cached`, the refresh only runs when the cache is missing or stale, so a
    /// dependency that stays unresolvable does not fetch once per query. The exception is a
    /// requested-but-absent cryptocurrency table: a listings outage is retried immediately
    /// instead of waiting out a whole [`currency_ttl`], since that state is exactly what a
    /// refetch can fill. Failures are logged and the next qualifying query retries.
    ///
    /// [`currency_ttl`]: Settings::currency_ttl
    async fn ensure_currency_cached(&self) {
        let Some(client) = &self.client else {
            return;
        };
        let Some(actor) = self.actor.as_ref() else {
            warn!("the rink actor is not running; skipping the currency refresh");

            return;
        };

        let refreshed = {
            let fetch = || {
                refresh_data(
                    client,
                    &self.currency,
                    self.cmc.as_deref(),
                    self.crypto_enabled,
                    &self.currency_url,
                    actor,
                )
            };

            // A missing or stale cache is fetched through `refresh`; only a requested table
            // that is still absent forces the fetch past a fresh entry.
            if self.crypto_table_pending() {
                self.currency.force_refresh(fetch).await.map(|_| ())
            } else {
                self.currency.refresh(fetch).await
            }
        };

        if let Err(error) = refreshed {
            warn!(%error, "could not refresh currency data");
        }
    }

    /// Returns whether a requested cryptocurrency table is absent from the cached dataset.
    ///
    /// `true` when the `crypto` setting is on and the coinmarketcap plugin shared its client,
    /// but nothing is cached yet or the cached entry carries no listings. Only in that state
    /// does an unconditional fetch pay off: the fiat data may be fresh while the table a query
    /// needs is still missing. In every other case the cache already holds what the context
    /// was built from, and re-fetching cannot resolve a pending dependency.
    fn crypto_table_pending(&self) -> bool {
        self.crypto_enabled
            && self.cmc.is_some()
            && self
                .currency
                .read(|data| data.is_none_or(|entry| entry.crypto.is_none()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;
    use zeta_utils::irc::strip_formatting;

    use crate::test_support::{
        TEST_CURRENCY_DATA, currency_server, evaluate, listings_server, offline_plugin,
        plugin_with_listings, test_client_config, test_plugin,
    };

    #[tokio::test]
    async fn ans_references_the_previous_result() {
        let rink = offline_plugin();

        evaluate(&rink, "4 m", false).await;

        let reply = strip_formatting(&evaluate(&rink, "ans * 2", false).await);
        assert!(reply.contains("8 meter"), "{reply}");
        assert!(reply.contains("length"), "{reply}");
    }

    #[tokio::test]
    async fn the_extra_units_load_and_resolve() {
        let rink = offline_plugin();

        let cases = [
            ("300 px -> mm", &["79.375 millimeter", "length"][..]),
            ("90 grad -> degree", &["81 degree", "angle"][..]),
            (
                "300 dpi -> dpcm",
                &["15000/127", "118.1102 dpcm", "m^-1"][..],
            ),
            ("1 dppx -> dpi", &["96 dpi", "m^-1"][..]),
        ];

        for (line, expected) in cases {
            let reply = strip_formatting(&evaluate(&rink, line, false).await);

            for needle in expected {
                assert!(
                    reply.contains(needle),
                    "{line}: missing {needle:?}: {reply}"
                );
            }
        }
    }

    #[tokio::test]
    async fn crypto_units_and_subunits_convert() {
        let (rink, _fiat, _listings) = plugin_with_listings(200).await;

        rink.ensure_currency_cached().await;

        for (line, classify) in [("1 ETH to USD", false), ("1 eth to usd", true)] {
            let reply = strip_formatting(&evaluate(&rink, line, classify).await);
            assert!(reply.contains("2650.32 USD"), "{line}: {reply}");
            assert!(reply.contains("money"), "{line}: {reply}");
        }

        // 50_000 satoshis are 0.0005 bitcoin at the fixture's 65 000 USD price.
        let sats = strip_formatting(&evaluate(&rink, "50000 sats to usd", true).await);
        assert!(sats.contains("32.5 USD"), "{sats}");

        // A gwei stays convertible without being resolvable as fiat.
        let gwei = strip_formatting(&evaluate(&rink, "1 gwei to usd", true).await);
        assert!(gwei.contains("USD"), "{gwei}");
    }

    #[tokio::test]
    async fn synthesis_skips_colliding_and_invalid_symbols() {
        let (rink, _fiat, _listings) = plugin_with_listings(200).await;

        rink.ensure_currency_cached().await;

        // The fake EUR coin (priced 1.2) must not shadow the bundled euro: 1 EUR is 0.5 USD
        // under the fiat fixture, and the rejected `1INCH` symbol must not break the load.
        let reply = strip_formatting(&evaluate(&rink, "1 EUR to USD", false).await);
        assert!(reply.contains("0.5 USD"), "{reply}");
        assert!(!reply.contains("1.2"), "{reply}");
    }

    #[test]
    fn a_duplicate_ticker_keeps_the_whole_context_build_loadable() {
        use coinmarketcap::model::{Envelope, Listing};

        // Two listings sharing a ticker: rink rejects a definitions batch that defines the
        // same name twice, and a rejected crypto batch used to fail the whole build — taking
        // the fiat data fetched alongside it down too.
        let text = r#"{
            "data": [
                {"id":1,"name":"Foo","symbol":"FOO","slug":"foo","cmc_rank":1,
                 "quote":[{"id":2781,"symbol":"USD","price":1.0}]},
                {"id":2,"name":"Bar","symbol":"FOO","slug":"bar","cmc_rank":2,
                 "quote":[{"id":2781,"symbol":"USD","price":2.0}]}
            ],
            "status":{"timestamp":"2026-09-29T00:00:00Z","error_code":"0",
                      "error_message":null,"elapsed":1,"credit_count":1}
        }"#;
        let parsed: Envelope<Vec<Listing>> =
            zeta_http::json::from_str(text).expect("the fixture should decode");
        let data = CurrencyData {
            fiat: TEST_CURRENCY_DATA.to_string(),
            crypto: parsed.data,
        };

        let ctx = build_context(Some(&data), None)
            .expect("a duplicate ticker must not fail the whole context build");

        // The fiat dataset landed, and the higher-ranked coin of the pair survived.
        assert!(ctx.lookup("USD").is_some(), "the fiat data must still load");
        assert!(
            ctx.lookup("FOO").is_some(),
            "the first-ranked coin must load"
        );
    }

    #[tokio::test]
    async fn missing_crypto_answers_missing_dependencies() {
        // No coinmarketcap client was published: the crypto units stay unloaded even though
        // the crypto setting is on. The subunits' pending dependency declaration surfaces ETH
        // in the missing-dependencies error.
        let mut rink = offline_plugin();
        rink.crypto_enabled = true;
        rink.cmc = None;
        rink.ensure_currency_cached().await;

        let disabled = strip_formatting(&evaluate(&rink, "1 eth to usd", true).await);
        assert!(disabled.contains("Missing dependencies: ETH"), "{disabled}");

        // The fiat dataset is fine; the listings endpoint is not. The refresh fails as a
        // whole, so the last known context — without crypto — stays in place, and the
        // pending dependency still surfaces ETH.
        let (rink, _fiat, _listings) = plugin_with_listings(500).await;
        rink.ensure_currency_cached().await;

        let failed = strip_formatting(&evaluate(&rink, "1 eth to usd", true).await);
        assert!(failed.contains("Missing dependencies: ETH"), "{failed}");
    }

    #[tokio::test]
    async fn a_pending_crypto_table_is_retried_inline() {
        let fiat = currency_server(200).await;
        let listings = listings_server(500).await;

        let mut rink = test_plugin(Some(format!("{}/data/currency.json", fiat.uri())));
        rink.cmc = Some(Arc::new(
            CmcClient::with_base("test-api-key", &test_client_config(), &listings.uri()).unwrap(),
        ));

        rink.ensure_currency_cached().await;
        assert_eq!(listings.received_requests().await.unwrap().len(), 1);

        // The listings fetch failed, so the requested table is absent even though the fiat
        // entry is fresh: unlike an unresolvable dependency, this state a refetch can fix,
        // so the query retries the listings inline instead of waiting out the whole TTL.
        let reply = evaluate(&rink, "1 gwei to usd", true).await;
        assert!(reply.contains("Missing dependencies"), "{reply}");

        assert_eq!(listings.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn missing_fiat_answers_missing_dependencies() {
        let server = currency_server(500).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));

        let failed = strip_formatting(&evaluate(&rink, "1 USD", false).await);
        assert!(failed.contains("Missing dependencies: USD"), "{failed}");

        let rink = offline_plugin();

        let disabled = strip_formatting(&evaluate(&rink, "1 USD", false).await);
        assert!(disabled.contains("Missing dependencies: USD"), "{disabled}");
    }

    #[tokio::test]
    async fn a_fresh_cache_does_not_refetch_for_an_unresolvable_dependency() {
        let server = currency_server(200).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));

        // The cold cache fetches once, as it always has.
        rink.ensure_currency_cached().await;
        assert_eq!(server.received_requests().await.unwrap().len(), 1);

        // `gwei` rests on the ETH dependency, which no coinmarketcap client can resolve
        // here, so it stays pending — but the cached dataset is fresh and re-fetching it
        // cannot add the missing table. The queries must not reach the network again.
        for _ in 0..3 {
            // `.c` case-corrects `usd`, so the pending `gwei` dependency is what surfaces.
            let reply = evaluate(&rink, "1 gwei to usd", true).await;

            assert!(reply.contains("Missing dependencies"), "{reply}");
        }

        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    #[ignore = "needs network access"]
    async fn the_live_dataset_serves_fiat_and_bitcoin_queries() {
        let rink = test_plugin(None);

        rink.ensure_currency_cached().await;

        let fiat = evaluate(&rink, "100 usd to eur", true).await;
        assert!(fiat.contains("euro") && fiat.contains("money"), "{fiat}");

        let bitcoin = evaluate(&rink, "1 btc to usd", true).await;
        assert!(
            bitcoin.contains("USD") && bitcoin.contains("money"),
            "{bitcoin}"
        );
    }

    #[tokio::test]
    async fn the_plugin_registers_its_commands() {
        let mut subscriptions = Subscriptions::new();

        <Rink as Registered>::new(
            &zeta_test_support::test_context(),
            &Settings::default(),
            &mut subscriptions,
        )
        .unwrap();

        assert!(subscriptions.commands().contains(&RINK));
        assert!(subscriptions.commands().contains(&CLASSIFY));
    }

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.currency);
            assert_eq!(settings.currency_ttl, Duration::from_hours(1));
            assert_eq!(settings.currency_url, currency::CURRENCY_URL);
        }
        deserialize: {
            "currency": false,
            "currency_ttl": "30min",
            "currency_url": "https://example.invalid/currency.json",
        } assert: {
            assert!(!settings.currency);
            assert_eq!(settings.currency_ttl, Duration::from_mins(30));
            assert_eq!(settings.currency_url, "https://example.invalid/currency.json");
        }
    }
}
