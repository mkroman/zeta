//! Crypto currency quotes via the CoinMarketCap API.
//!
//! Provides the `.cc` command for quoting an arbitrary coin by symbol or (fuzzy) name, as well
//! as fixed triggers such as `.btc` and `.eth` that quote a known symbol. Both accept an
//! optional fiat currency to convert the price into (e.g. `.btc eur`).
//!
//! Name lookups are served from a cache of the top 1000 cryptocurrencies by market cap,
//! fetched from the cryptocurrency map endpoint and refreshed lazily every 24 hours. Symbols
//! of any coin resolve regardless, but coins ranked below the cached top 1000 do not resolve
//! by name.
//!
//! Fiat currencies for price conversion are fetched from the fiat map endpoint and cached the
//! same way; while the fiat cache is empty, validation of the optional currency argument is
//! deferred to the API.
//!
//! The CoinMarketCap API key is set in `[plugins.coinmarketcap]`, falling back to the
//! `COINMARKETCAP_API_KEY` environment variable; a missing key fails plugin initialization and
//! the plugin is skipped at startup. The `default_currency` (USD), `cache_ttl` (24 hours), and
//! `max_name_typos` (2) settings tune quoting and fuzzy name lookups.
//!
//! The module is split by concern: `args` holds the command-line types, `cache` the coin and
//! fiat caches with their lookups, and `commands` the command handlers and their reply mapping;
//! `format` formats a quote as an IRC message, and `matching` resolves a name fuzzily. This
//! module holds the plugin, its settings, and the command routing.

mod args;
mod cache;
mod commands;
mod format;
mod matching;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use zeta_cache::TtlCache;
use zeta_plugin::{
    Client, CommandEvent, Context, Subscriptions, ZetaError, plugin, plugin_err, resolve_secret,
};

use coinmarketcap::model::DEFAULT_CURRENCY;
use coinmarketcap::{Client as CmcClient, ClientOptions};

use self::cache::{CoinCache, FiatCache};
use self::commands::{CC, COIN_COMMANDS};

/// Settings for the coinmarketcap plugin, from its `[plugins.coinmarketcap]` configuration
/// section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// The CoinMarketCap API key.
    ///
    /// Falls back to the `COINMARKETCAP_API_KEY` environment variable when unset.
    pub api_key: Option<String>,
    /// The fiat currency used when a command does not specify one.
    pub default_currency: String,
    /// How long the cached coins and fiat currencies stay valid before being refreshed.
    #[serde(with = "humantime_serde")]
    pub cache_ttl: Duration,
    /// The maximum number of typos (needle characters missing from the name) allowed when
    /// fuzzy matching a coin name.
    pub max_name_typos: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            api_key: None,
            default_currency: DEFAULT_CURRENCY.to_string(),
            cache_ttl: Duration::from_hours(24),
            max_name_typos: 2,
        }
    }
}

/// Crypto currency quotes plugin, backed by the CoinMarketCap API.
#[derive(Debug)]
pub struct CoinMarketCap {
    /// Client for CoinMarketCap API requests, with the API key set as a default header.
    /// Shared with other plugins through [`Context::shared`].
    client: Arc<CmcClient>,
    /// The top cryptocurrencies by market cap, cached for the cache TTL.
    coins: TtlCache<CoinCache>,
    /// The fiat currencies supported for price conversion, cached for the cache TTL.
    fiat: TtlCache<FiatCache>,
    /// The fiat currency used when a command does not specify one.
    default_currency: String,
    /// The maximum number of typos allowed when fuzzy matching a coin name.
    max_name_typos: u16,
}

#[plugin(name = "coinmarketcap", settings = Settings)]
impl CoinMarketCap {
    fn new(
        ctx: &Context,
        settings: &Settings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, ZetaError> {
        let api_key = resolve_secret(settings.api_key.as_deref(), "COINMARKETCAP_API_KEY")?;
        let mut options = ClientOptions::default();
        options.timeout = Some(ctx.config().http().timeout);
        options.user_agent = Some(ctx.config().http().user_agent.clone());
        let client =
            Arc::new(CmcClient::try_new_with_options(api_key, &options).map_err(plugin_err)?);

        // The keyed CoinMarketCap client is published for other plugins to reuse, e.g. the
        // rink plugin's live cryptocurrency units.
        ctx.shared().publish(Arc::clone(&client));

        for (command, _) in COIN_COMMANDS {
            subscriptions.command(*command);
        }

        subscriptions.command(CC);

        Ok(Self {
            client,
            coins: TtlCache::new(settings.cache_ttl),
            fiat: TtlCache::new(settings.cache_ttl),
            default_currency: settings.default_currency.clone(),
            max_name_typos: settings.max_name_typos,
        })
    }

    #[handler(loaded)]
    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        self.ensure_coins_cached().await;
        self.ensure_fiat_cached().await;

        Ok(())
    }

    #[handler(command)]
    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        if command.spec == CC {
            return self
                .handle_cc(client, command.channel(), command.args())
                .await;
        }

        if let Some((_, symbol)) = COIN_COMMANDS.iter().find(|(spec, _)| *spec == command.spec) {
            return self
                .handle_coin(
                    command.spec,
                    symbol,
                    client,
                    command.channel(),
                    command.args(),
                )
                .await;
        }

        Ok(())
    }
}
