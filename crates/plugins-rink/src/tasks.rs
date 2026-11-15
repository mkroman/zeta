//! The actor and currency refresh tasks.
//!
//! The rink context is moved into a dedicated actor when the plugin loads, and a background
//! task rebuilds it from freshly fetched currency data once per `currency_ttl`. A query that
//! runs into missing currency data asks for an inline refresh before it retries.

use std::sync::Arc;

use tokio::time::MissedTickBehavior;
use tracing::{Instrument, debug, warn};

use zeta_plugin::Chain;

use super::Rink;
use super::actor::Handle;
use super::currency::{CURRENCY_CHECK_INTERVAL, refresh_data};

impl Rink {
    /// Moves the engine into the actor task that serializes evaluations.
    pub(super) fn spawn_actor(&mut self) {
        let engine = self.engine.take().expect("the rink context is built once");

        self.actor = Some(Handle::spawn(engine));
    }

    /// Starts the background task that refreshes the currency data at the TTL.
    ///
    /// The task fetches on its first tick and again whenever the cached data expires, asking the
    /// actor to swap in a context rebuilt with the new rates. Requires the `currency` setting to
    /// have created an HTTP client and the actor to be running.
    pub(super) fn spawn_refresh_task(&mut self) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(actor) = self.actor.clone() else {
            return;
        };

        let (currency, url) = (Arc::clone(&self.currency), self.currency_url.clone());
        let (cmc, crypto_enabled) = (self.cmc.clone(), self.crypto_enabled);

        self.refresh_task = Some(tokio::spawn(async move {
            tracing::info_span!("currency.refresh.start").in_scope(|| {
                debug!("starting currency refresh task");
            });

            let mut interval = tokio::time::interval(CURRENCY_CHECK_INTERVAL);
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            loop {
                interval.tick().await;

                async {
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
                        warn!(
                            exception.message = %Chain(&error),
                            "could not refresh currency data"
                        );
                    }
                }
                // The target stays under `zeta`: the tick parents the HTTP calls, and a plugin
                // log filter must not leave them as trace roots of their own.
                .instrument(tracing::info_span!(
                    target: "zeta.plugins",
                    "currency.refresh"
                ))
                .await;
            }
        }));
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
    /// [`currency_ttl`]: crate::Settings::currency_ttl
    pub(super) async fn ensure_currency_cached(&self) {
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
            warn!(
                exception.message = %Chain(&error),
                "could not refresh currency data"
            );
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
                .with_value(|data| data.is_none_or(|entry| entry.crypto.is_none()))
    }
}
