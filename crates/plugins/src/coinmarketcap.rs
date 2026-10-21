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

mod command;
mod name;
mod reply;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use self::command::{
    CC, CC_USAGE, COIN_COMMANDS, COMMANDS, CoinOptions, QuoteOptions, parse_currency,
};
use self::reply::format_quote;
use crate::prelude::*;
use zeta_cache::TtlCache;

use coinmarketcap::model::{Coin, CoinQuery, DEFAULT_CURRENCY, Fiat, QuoteData};
use coinmarketcap::{Client as CmcClient, ClientOptions, Error};

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

/// The top cryptocurrencies by market cap, keyed by ticker symbol.
///
/// Populated from the cryptocurrency map endpoint at startup and refreshed lazily after the
/// cache TTL expires.
#[derive(Debug)]
struct CoinCache {
    /// The cached coins, keyed by ticker symbol.
    by_symbol: HashMap<String, Coin>,
    /// The lowercased coin names, with the symbol each belongs to, for fuzzy name lookups.
    by_name: Vec<(String, String)>,
}

impl From<Vec<Coin>> for CoinCache {
    fn from(coins: Vec<Coin>) -> Self {
        // The map endpoint returns coins in ascending rank order; keep the first (highest
        // ranked) occurrence when multiple coins share a symbol.
        let mut by_symbol = HashMap::with_capacity(coins.len());
        let mut by_name = Vec::with_capacity(coins.len());

        for coin in coins {
            if by_symbol.contains_key(&coin.symbol) {
                continue;
            }

            by_name.push((coin.name.to_lowercase(), coin.symbol.clone()));
            by_symbol.insert(coin.symbol.clone(), coin);
        }

        Self { by_symbol, by_name }
    }
}

/// The fiat currencies supported by the API for price conversion.
///
/// Populated from the fiat map endpoint at startup and refreshed lazily after the cache TTL
/// expires.
#[derive(Debug)]
struct FiatCache {
    /// The valid fiat currency symbols (e.g. `USD`).
    symbols: HashSet<String>,
    /// The currency sign of each symbol (e.g. `USD` maps to `$`).
    signs: HashMap<String, String>,
}

impl From<Vec<Fiat>> for FiatCache {
    fn from(fiats: Vec<Fiat>) -> Self {
        Self {
            symbols: fiats.iter().map(|fiat| fiat.symbol.clone()).collect(),
            signs: fiats
                .into_iter()
                .map(|fiat| (fiat.symbol, fiat.sign))
                .collect(),
        }
    }
}

/// Crypto currency quotes plugin, backed by the CoinMarketCap API.
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

#[async_trait]
impl Plugin<Context> for CoinMarketCap {
    type Settings = Settings;

    fn new(
        ctx: &Context,
        settings: &Settings,
        subscriptions: &mut Subscriptions,
    ) -> Result<Self, PluginError> {
        let api_key = resolve_secret(settings.api_key.as_deref(), "COINMARKETCAP_API_KEY")?;
        let client = Arc::new(
            CmcClient::with_options(
                api_key,
                &ClientOptions {
                    timeout: ctx.config.http.timeout,
                    user_agent: ctx.config.http.user_agent.clone(),
                },
            )
            .map_err(plugin_err)?,
        );

        // The keyed CoinMarketCap client is published for other plugins to reuse, e.g. the
        // rink plugin's live cryptocurrency units.
        ctx.shared.publish(Arc::clone(&client));

        for command in COMMANDS {
            subscriptions.subscribe_command(*command);
        }

        Ok(Self {
            client,
            coins: TtlCache::new(settings.cache_ttl),
            fiat: TtlCache::new(settings.cache_ttl),
            default_currency: settings.default_currency.clone(),
            max_name_typos: settings.max_name_typos,
        })
    }

    async fn start(&mut self, _ctx: &Context, _client: &IrcClient) -> Result<(), PluginError> {
        self.ensure_coins_cached().await;
        self.ensure_fiat_cached().await;

        Ok(())
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &IrcClient,
        command: &CommandEvent,
    ) -> Result<(), PluginError> {
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

impl CoinMarketCap {
    /// Handles a `.cc` invocation, resolving `args` to a coin by symbol or fuzzy name.
    async fn handle_cc(
        &self,
        client: &IrcClient,
        channel: &str,
        args: &str,
    ) -> Result<(), PluginError> {
        let options = match CC.parse_args::<CoinOptions>(args) {
            Ok(options) if !options.coin.trim().is_empty() => options,
            _ => {
                client.send_privmsg(channel, plain_reply(CC_USAGE))?;
                return Ok(());
            }
        };

        self.ensure_coins_cached().await;

        // Prefer a cached coin (by symbol, then by fuzzy name); fall back to letting the API
        // resolve the query as a symbol (e.g. when the cache has not been populated yet, or the
        // coin is outside the cached top 1000).
        let query = self.resolve_coin(&options.coin).map_or_else(
            || CoinQuery::Symbol(options.coin.to_ascii_uppercase()),
            CoinQuery::Id,
        );

        self.quote_and_reply(client, channel, query, options.currency.as_deref())
            .await
    }

    /// Handles a fixed coin command invocation (e.g. `.btc`), quoting the given `symbol`.
    async fn handle_coin(
        &self,
        command: CommandSpec,
        symbol: &str,
        client: &IrcClient,
        channel: &str,
        args: &str,
    ) -> Result<(), PluginError> {
        let Ok(options) = command.parse_args::<QuoteOptions>(args) else {
            client.send_privmsg(
                channel,
                plain_reply(format!("Usage: {} \x0f[currency]", command.trigger())),
            )?;
            return Ok(());
        };

        self.quote_and_reply(
            client,
            channel,
            CoinQuery::Symbol(symbol.to_string()),
            options.currency.as_deref(),
        )
        .await
    }

    /// Validates and resolves the optional conversion currency, quotes `query` in it and
    /// replies to `channel` with the result.
    async fn quote_and_reply(
        &self,
        client: &IrcClient,
        channel: &str,
        query: CoinQuery,
        currency: Option<&str>,
    ) -> Result<(), PluginError> {
        self.ensure_fiat_cached().await;

        let currency = match parse_currency(currency, &self.default_currency, |symbol| {
            self.is_valid_currency(symbol)
        }) {
            Ok(currency) => currency,
            Err(err) => return Self::reply_error(client, channel, &err),
        };

        let quote = self.client.get_quote(query, &currency).await;

        Self::reply_quote(
            client,
            channel,
            quote,
            &currency,
            self.fiat_sign(&currency).as_deref(),
        )
    }

    /// Replies with the result of a coin quote lookup.
    fn reply_quote(
        client: &IrcClient,
        channel: &str,
        quote: Result<QuoteData, Error>,
        currency: &str,
        sign: Option<&str>,
    ) -> Result<(), PluginError> {
        match quote {
            Ok(quote) => client.send_privmsg(channel, format_quote(&quote, currency, sign))?,
            Err(err) => Self::reply_error(client, channel, &err)?,
        }

        Ok(())
    }

    /// Replies with the message of a failed coin lookup, logging unexpected failures.
    fn reply_error(client: &IrcClient, channel: &str, err: &Error) -> Result<(), PluginError> {
        // Unknown coins and unsupported currencies are user errors: they are replied to
        // verbatim, without logging. Anything else is unexpected and worth a warning.
        if !matches!(err, Error::NotFound | Error::InvalidCurrency(_)) {
            warn!(error = %err, "coin lookup failed");
        }

        client.send_privmsg(channel, plain_reply(err.to_string()))?;

        Ok(())
    }

    /// Resolves a user query to a coin id, by exact symbol match first and by fuzzy name match
    /// second.
    fn resolve_coin(&self, query: &str) -> Option<u64> {
        self.coins.read(|cache| {
            let cache = cache?;

            cache
                .by_symbol
                .get(&query.to_ascii_uppercase())
                .or_else(|| cache.find_by_name(query, self.max_name_typos))
                .map(|coin| coin.id)
        })
    }

    /// Refreshes the coin cache when it is missing or older than the cache TTL.
    ///
    /// Failures are logged and leave the existing cache, if any, in place: names keep
    /// resolving with the stale coins, and an empty cache falls back to letting the API
    /// resolve queries as symbols.
    async fn ensure_coins_cached(&self) {
        if let Err(err) = self
            .coins
            .refresh(|| async {
                let coins = self.client.coin_map().await?;
                let count = coins.len();
                debug!(count, "cached the top cryptocurrencies");

                Ok::<_, Error>(CoinCache::from(coins))
            })
            .await
        {
            warn!(error = %err, "could not cache the top cryptocurrencies");
        }
    }

    /// Refreshes the fiat currency cache when it is missing or older than the cache TTL.
    ///
    /// Failures are logged and leave the existing cache, if any, in place: quotes keep working
    /// with the stale currencies, and an empty cache defers currency validation to the API.
    async fn ensure_fiat_cached(&self) {
        if let Err(err) = self
            .fiat
            .refresh(|| async {
                let fiats = self.client.fiat_map().await?;
                let count = fiats.len();
                debug!(count, "cached the supported fiat currencies");

                Ok::<_, Error>(FiatCache::from(fiats))
            })
            .await
        {
            warn!(error = %err, "could not cache the supported fiat currencies");
        }
    }

    /// Returns whether `currency` is a fiat currency supported for price conversion.
    ///
    /// Returns `true` while the fiat cache has not been populated, deferring validation to
    /// the API.
    fn is_valid_currency(&self, currency: &str) -> bool {
        self.fiat.read(|cache| {
            cache.is_none_or(|cache| cache.symbols.is_empty() || cache.symbols.contains(currency))
        })
    }

    /// Returns the currency sign for `currency`, if known.
    fn fiat_sign(&self, currency: &str) -> Option<String> {
        self.fiat
            .read(|cache| cache.and_then(|cache| cache.signs.get(currency).cloned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;

    /// Builds a coin list for cache tests.
    fn test_coins() -> Vec<Coin> {
        vec![
            Coin {
                id: 1,
                name: "Bitcoin".to_string(),
                symbol: "BTC".to_string(),
            },
            Coin {
                id: 2,
                name: "Bitcoin Cash".to_string(),
                symbol: "BCH".to_string(),
            },
            Coin {
                id: 1027,
                name: "Ethereum".to_string(),
                symbol: "ETH".to_string(),
            },
            Coin {
                id: 74,
                name: "Dogecoin".to_string(),
                symbol: "DOGE".to_string(),
            },
        ]
    }

    /// Builds a plugin instance for lookup tests, without touching the API.
    fn test_plugin() -> CoinMarketCap {
        CoinMarketCap {
            client: Arc::new(
                CmcClient::with_options("test-api-key", &ClientOptions::default()).unwrap(),
            ),
            coins: TtlCache::with_value(CoinCache::from(test_coins()), CACHE_TTL),
            fiat: TtlCache::new(CACHE_TTL),
            default_currency: DEFAULT_CURRENCY.to_string(),
            max_name_typos: 2,
        }
    }

    #[tokio::test]
    async fn publishes_its_client_for_other_plugins() {
        // The keyed CoinMarketCap client must be discoverable through the shared state, e.g.
        // by the rink plugin's live cryptocurrency units.
        let ctx = zeta_test_support::test_context();
        let mut subscriptions = Subscriptions::new();
        let settings = Settings {
            api_key: Some("test-api-key".to_string()),
            ..Settings::default()
        };

        <CoinMarketCap as Plugin<Context>>::new(&ctx, &settings, &mut subscriptions).unwrap();

        assert!(ctx.shared.get::<CmcClient>().is_some());
    }

    /// The default cache lifetime, as configured in [`Settings::default`].
    const CACHE_TTL: Duration = Duration::from_hours(24);

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.api_key.is_none());
            assert_eq!(settings.default_currency, "USD");
            assert_eq!(settings.cache_ttl, Duration::from_hours(24));
            assert_eq!(settings.max_name_typos, 2);
        }
        deserialize: {
            "api_key": "secret",
            "default_currency": "eur",
            "cache_ttl": "1h",
            "max_name_typos": 3,
        } assert: {
            assert_eq!(settings.api_key.as_deref(), Some("secret"));
            assert_eq!(settings.default_currency, "eur");
            assert_eq!(settings.cache_ttl, Duration::from_hours(1));
            assert_eq!(settings.max_name_typos, 3);
        }
    }

    #[test]
    fn keeps_highest_ranked_coin_for_duplicate_symbols() {
        // The cache is built from a rank-ordered list, so the first occurrence of a symbol
        // (the highest ranked coin) must win.
        let cache = CoinCache::from(vec![
            Coin {
                id: 336,
                name: "ONEchain".to_string(),
                symbol: "ONE".to_string(),
            },
            Coin {
                id: 924,
                name: "Harmony".to_string(),
                symbol: "ONE".to_string(),
            },
        ]);

        assert_eq!(cache.by_symbol["ONE"].id, 336);
    }

    #[test]
    fn resolves_coins_by_symbol_and_name() {
        let plugin = test_plugin();

        assert_eq!(plugin.resolve_coin("btc"), Some(1));

        // `bitcoin` matches `Bitcoin` rather than the longer `Bitcoin Cash`.
        assert_eq!(plugin.resolve_coin("bitcoin"), Some(1));

        // Misspelled names still resolve, including with substituted characters.
        assert_eq!(plugin.resolve_coin("bitcon"), Some(1));
        assert_eq!(plugin.resolve_coin("bitkoin"), Some(1));
        assert_eq!(plugin.resolve_coin("doge"), Some(74));

        assert!(plugin.resolve_coin("").is_none());
        assert!(plugin.resolve_coin("   ").is_none());
        assert!(plugin.resolve_coin("zzzz").is_none());
    }

    #[test]
    fn validates_currencies_against_the_fiat_cache() {
        let mut plugin = test_plugin();

        // An empty fiat cache defers validation to the API.
        assert!(plugin.is_valid_currency("XYZ"));

        plugin.fiat = TtlCache::with_value(
            FiatCache::from(vec![Fiat {
                sign: "$".to_string(),
                symbol: "USD".to_string(),
            }]),
            CACHE_TTL,
        );

        assert!(plugin.is_valid_currency("USD"));
        assert!(!plugin.is_valid_currency("XYZ"));
    }
}
