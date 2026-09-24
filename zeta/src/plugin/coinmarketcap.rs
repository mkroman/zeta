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

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use argh::{ArgsInfo, FromArgs};
use serde::{Deserialize, Serialize};
use strsim::jaro_winkler;
use tracing::{debug, warn};

use crate::{
    cache::TtlCache,
    plugin::prelude::*,
    utils::strip_control_chars,
};

mod client;
mod error;
mod model;

use error::Error;
use model::{Coin, CoinQuery, DEFAULT_CURRENCY, Fiat, QuoteData};

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

/// The `.cc` command.
const CC: CommandSpec = CommandSpec::with_args::<CoinOpts>(".cc", "Quote any coin by symbol or fuzzy name");

/// The `.btc` command.
const BTC: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".btc", "Quote Bitcoin (BTC) in a fiat currency");
/// The `.eth` command.
const ETH: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".eth", "Quote Ethereum (ETH) in a fiat currency");
/// The `.zcash` command.
const ZCASH: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".zcash", "Quote Zcash (ZEC) in a fiat currency");
/// The `.zec` command.
const ZEC: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".zec", "Quote Zcash (ZEC) in a fiat currency");
/// The `.ans` command (Antshares, the former name of Neo).
const ANS: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".ans", "Quote Neo (NEO), formerly Antshares");
/// The `.neo` command.
const NEO: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".neo", "Quote Neo (NEO) in a fiat currency");
/// The `.stellar` command.
const STELLAR: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".stellar", "Quote Stellar (XLM) in a fiat currency");
/// The `.xmr` command.
const XMR: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".xmr", "Quote Monero (XMR) in a fiat currency");
/// The `.xrp` command.
const XRP: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".xrp", "Quote XRP in a fiat currency");
/// The `.ltc` command.
const LTC: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".ltc", "Quote Litecoin (LTC) in a fiat currency");
/// The `.etc` command.
const ETC: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".etc", "Quote Ethereum Classic (ETC) in a fiat currency");
/// The `.golem` command.
const GOLEM: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".golem", "Quote Golem (GNT) in a fiat currency");
/// The `.sia` command.
const SIA: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".sia", "Quote Siacoin (SC) in a fiat currency");
/// The `.doge` command.
const DOGE: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".doge", "Quote Dogecoin (DOGE) in a fiat currency");
/// The `.maid` command.
const MAID: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".maid", "Quote MaidSafeCoin (MAID) in a fiat currency");
/// The `.bcash` command.
const BCASH: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".bcash", "Quote Bitcoin Cash (BCH) in a fiat currency");
/// The `.trump` command.
const TRUMP: CommandSpec = CommandSpec::with_args::<QuoteOpts>(".trump", "Quote Trump (TRUMP) in a fiat currency");

/// Fixed coin commands, mapped to the symbol they quote.
const COIN_COMMANDS: &[(CommandSpec, &str)] = &[
    (BTC, "BTC"),
    (ETH, "ETH"),
    (ZCASH, "ZEC"),
    (ZEC, "ZEC"),
    (ANS, "NEO"),
    (NEO, "NEO"),
    (STELLAR, "XLM"),
    (XMR, "XMR"),
    (XRP, "XRP"),
    (LTC, "LTC"),
    (ETC, "ETC"),
    (GOLEM, "GNT"),
    (SIA, "SC"),
    (DOGE, "DOGE"),
    (MAID, "MAID"),
    (BCASH, "BCH"),
    (TRUMP, "TRUMP"),
];

/// The commands handled by this plugin.
const COMMANDS: &[CommandSpec] = &[
    CC, BTC, ETH, ZCASH, ZEC, ANS, NEO, STELLAR, XMR, XRP, LTC, ETC, GOLEM, SIA, DOGE, MAID, BCASH,
    TRUMP,
];

/// Convert a coin price into another fiat currency.
#[derive(FromArgs, ArgsInfo, Debug)]
struct QuoteOpts {
    /// fiat currency to convert the price into (defaults to USD)
    #[argh(positional)]
    currency: Option<String>,
}

/// The usage hint for the `.cc` command.
const CC_USAGE: &str = "Usage: .cc \x0f<symbol> [currency]";

/// Convert a cryptocurrency price into fiat currency.
#[derive(FromArgs, ArgsInfo, Debug)]
struct CoinOpts {
    /// the coin to quote, by symbol or name
    #[argh(positional)]
    coin: String,
    /// fiat currency to convert the price into (defaults to USD)
    #[argh(positional)]
    currency: Option<String>,
}

/// The top cryptocurrencies by market cap, keyed by ticker symbol.
///
/// Populated from the cryptocurrency map endpoint at startup and refreshed lazily after the
/// cache TTL expires.
#[derive(Debug)]
struct CoinCache {
    /// The cached coins, keyed by ticker symbol.
    by_symbol: HashMap<String, Coin>,
}

impl From<Vec<Coin>> for CoinCache {
    fn from(coins: Vec<Coin>) -> Self {
        // The map endpoint returns coins in ascending rank order; keep the first (highest
        // ranked) occurrence when multiple coins share a symbol.
        let mut by_symbol = HashMap::with_capacity(coins.len());

        for coin in coins {
            by_symbol.entry(coin.symbol.clone()).or_insert(coin);
        }

        Self { by_symbol }
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
    client: client::Client,
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

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Self, ZetaError> {
        let api_key = resolve_secret(settings.api_key.as_deref(), "COINMARKETCAP_API_KEY")?;
        let client = client::Client::new(&api_key, &ctx.config.http)?;

        for command in COMMANDS {
            subscriptions.command(*command);
        }

        Ok(Self {
            client,
            coins: TtlCache::new(settings.cache_ttl),
            fiat: TtlCache::new(settings.cache_ttl),
            default_currency: settings.default_currency.clone(),
            max_name_typos: settings.max_name_typos,
        })
    }

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        self.ensure_coins_cached().await;
        self.ensure_fiat_cached().await;

        Ok(())
    }

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

        if let Some((_, symbol)) = COIN_COMMANDS
            .iter()
            .find(|(spec, _)| *spec == command.spec)
        {
            return self
                .handle_coin(command.spec, symbol, client, command.channel(), command.args())
                .await;
        }

        Ok(())
    }
}

impl CoinMarketCap {
    /// Handles a `.cc` invocation, resolving `args` to a coin by symbol or fuzzy name.
    async fn handle_cc(&self, client: &Client, channel: &str, args: &str) -> Result<(), ZetaError> {
        let opts = match CC.parse_args::<CoinOpts>(args) {
            Ok(opts) if !opts.coin.trim().is_empty() => opts,
            _ => {
                client.send_privmsg(channel, notice(CC_USAGE))?;
                return Ok(());
            }
        };

        self.ensure_coins_cached().await;

        // Prefer a cached coin (by symbol, then by fuzzy name); fall back to letting the API
        // resolve the query as a symbol (e.g. when the cache has not been populated yet, or the
        // coin is outside the cached top 1000).
        let query = match self.resolve_coin(&opts.coin) {
            Some(coin) => CoinQuery::Id(coin.id),
            None => CoinQuery::Symbol(opts.coin.to_ascii_uppercase()),
        };

        self.quote_and_reply(client, channel, query, opts.currency.as_deref())
            .await
    }

    /// Handles a fixed coin command invocation (e.g. `.btc`), quoting the given `symbol`.
    async fn handle_coin(
        &self,
        command: CommandSpec,
        symbol: &str,
        client: &Client,
        channel: &str,
        args: &str,
    ) -> Result<(), ZetaError> {
        let Ok(opts) = command.parse_args::<QuoteOpts>(args) else {
            client.send_privmsg(
                channel,
                notice(format!("Usage: {} \x0f[currency]", command.trigger())),
            )?;
            return Ok(());
        };

        self.quote_and_reply(
            client,
            channel,
            CoinQuery::Symbol(symbol.to_string()),
            opts.currency.as_deref(),
        )
        .await
    }

    /// Validates and resolves the optional conversion currency, quotes `query` in it and
    /// replies to `channel` with the result.
    async fn quote_and_reply(
        &self,
        client: &Client,
        channel: &str,
        query: CoinQuery,
        currency: Option<&str>,
    ) -> Result<(), ZetaError> {
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
        client: &Client,
        channel: &str,
        quote: Result<QuoteData, Error>,
        currency: &str,
        sign: Option<&str>,
    ) -> Result<(), ZetaError> {
        match quote {
            Ok(quote) => client.send_privmsg(channel, format_quote(&quote, currency, sign))?,
            Err(err) => Self::reply_error(client, channel, &err)?,
        }

        Ok(())
    }

    /// Replies with the message of a failed coin lookup, logging unexpected failures.
    fn reply_error(client: &Client, channel: &str, err: &Error) -> Result<(), ZetaError> {
        // Unknown coins and unsupported currencies are user errors: they are replied to
        // verbatim, without logging. Anything else is unexpected and worth a warning.
        if !matches!(err, Error::NotFound | Error::InvalidCurrency(_)) {
            warn!(error = %err, "coin lookup failed");
        }

        client.send_privmsg(channel, notice(err.to_string()))?;

        Ok(())
    }

    /// Resolves a user query to a coin, by exact symbol match first and by fuzzy name match
    /// second.
    fn resolve_coin(&self, query: &str) -> Option<Coin> {
        self.coins.read(|cache| {
            let coins = cache.map(|cache| &cache.by_symbol)?;

            coins
                .get(&query.to_ascii_uppercase())
                .cloned()
                .or_else(|| find_by_name(coins, query, self.max_name_typos))
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
            cache.is_none_or(|cache| {
                cache.symbols.is_empty() || cache.symbols.contains(currency)
            })
        })
    }

    /// Returns the currency sign for `currency`, if known.
    fn fiat_sign(&self, currency: &str) -> Option<String> {
        self.fiat
            .read(|cache| cache.and_then(|cache| cache.signs.get(currency).cloned()))
    }
}

/// Parses and validates the optional conversion currency of a command invocation.
///
/// The input is sanitized before validation, so an unsupported currency can be echoed back to
/// the channel without leaking IRC formatting control characters. Validation is delegated to
/// `is_valid`, which accepts everything while the fiat cache has not been populated,
/// deferring validation to the API.
fn parse_currency(
    currency: Option<&str>,
    default_currency: &str,
    is_valid: impl Fn(&str) -> bool,
) -> Result<String, Error> {
    let currency =
        strip_control_chars(currency.unwrap_or(default_currency)).to_ascii_uppercase();

    if is_valid(&currency) {
        Ok(currency)
    } else {
        Err(Error::InvalidCurrency(currency))
    }
}

/// Finds the best fuzzy name match for `query` among `coins`, if any.
///
/// A candidate matches when its name contains the query's characters in order, with at most
/// `max_name_typos` of them left unmatched. Names containing the query verbatim are preferred
/// (exact match first, then prefix, then substring); the rest are scored by Jaro-Winkler
/// similarity, scaled below verbatim matches. The highest score wins, preferring the shorter
/// name on ties (e.g. `Bitcoin` over `Bitcoin Cash` for the query `bitcoin`).
fn find_by_name(coins: &HashMap<String, Coin>, query: &str, max_name_typos: u16) -> Option<Coin> {
    let needle = query.trim().to_lowercase();

    if needle.is_empty() {
        return None;
    }

    let max_typos = usize::from(max_name_typos);

    coins
        .values()
        .filter_map(|coin| {
            let name = coin.name.to_lowercase();

            score_name(&needle, &name, max_typos).map(|score| (score, coin))
        })
        .max_by(|(a_score, a_coin), (b_score, b_coin)| {
            a_score
                .total_cmp(b_score)
                .then_with(|| b_coin.name.len().cmp(&a_coin.name.len()))
                .then_with(|| a_coin.symbol.cmp(&b_coin.symbol))
        })
        .map(|(_, coin)| coin.clone())
}

/// Scores how well `name` matches `needle` (both lowercased), or `None` if it does not match.
///
/// Verbatim matches rank above fuzzy ones: exact (`1.0`), prefix (`0.95`), then substring
/// (`0.9`). Remaining candidates must match with at most `max_typos` unmatched characters, and
/// are scored by Jaro-Winkler similarity scaled below verbatim matches (Jaro's matching window
/// ignores characters near the end of longer names, so e.g. `gold` must not rely on it to find
/// `PAX Gold`).
fn score_name(needle: &str, name: &str, max_typos: usize) -> Option<f64> {
    if name == needle {
        return Some(1.0);
    }

    if name.starts_with(needle) {
        return Some(0.95);
    }

    if name.contains(needle) {
        return Some(0.9);
    }

    matches_within(needle, name, max_typos).then(|| jaro_winkler(needle, name) * 0.85)
}

/// Returns whether `needle` can be matched in `haystack`, in order, with at most `max_typos`
/// of its characters left unmatched.
fn matches_within(needle: &str, haystack: &str, max_typos: usize) -> bool {
    let needle: Vec<char> = needle.chars().collect();
    let haystack: Vec<char> = haystack.chars().collect();

    // The number of unmatched needle characters is the needle length minus the length of the
    // longest common subsequence, computed with the usual row-by-row dynamic program.
    let mut previous = vec![0; haystack.len() + 1];
    let mut current = vec![0; haystack.len() + 1];

    for &needle_char in &needle {
        for (index, &haystack_char) in haystack.iter().enumerate() {
            current[index + 1] = if needle_char == haystack_char {
                previous[index] + 1
            } else {
                current[index].max(previous[index + 1])
            };
        }

        std::mem::swap(&mut previous, &mut current);
    }

    needle.len() - previous[haystack.len()] <= max_typos
}

/// Formats a coin quote as an IRC message, e.g.:
///
/// `\x0310> Bitcoin (\x0fBTC\x0310) is currently trading at\x0f $97231.50\x0310 (...)`
fn format_quote(quote: &QuoteData, currency: &str, sign: Option<&str>) -> String {
    let name = &quote.name;
    let symbol = &quote.symbol;

    let Some(fiat) = quote.quote.get(currency) else {
        return notice(format!(
            "{name} ({RESET}{symbol}{COLOR}) has no quote in {currency}"
        ));
    };

    let price = fiat.price.map_or_else(
        || "n/a".to_string(),
        |price| format_money(price, currency, sign),
    );

    let change_hour = format_change(fiat.percent_change_1h);
    let change_day = format_change(fiat.percent_change_24h);
    let change_week = format_change(fiat.percent_change_7d);

    notice(format!(
        "{name} ({RESET}{symbol}{COLOR}) is currently trading at{RESET} {price}{COLOR} \
         (1 Hour Change:{RESET} {change_hour}{COLOR} 24 Hour Change:{RESET} {change_day}{COLOR} \
         7 Day Change:{RESET} {change_week}{COLOR})"
    ))
}

/// Formats a price change percentage in green (positive) or red (negative), followed by a reset.
fn format_change(change: Option<f64>) -> String {
    let Some(change) = change else {
        return "\x0fn/a".to_string();
    };

    if change.is_sign_negative() {
        format!("\x034{change:+.2}%\x0f")
    } else {
        format!("\x033{change:+.2}%\x0f")
    }
}

/// Formats a price with the currency sign when one is known, appending the currency code
/// otherwise.
fn format_money(price: f64, currency: &str, sign: Option<&str>) -> String {
    let price = format_price(price);

    sign.map_or_else(
        || format!("{price} {currency}"),
        |sign| format!("{sign}{price}"),
    )
}

/// Formats a price with two decimals when it is at least one unit, and with up to ten decimals
/// (trailing zeroes trimmed) otherwise, to preserve precision for micro-priced coins.
fn format_price(price: f64) -> String {
    if price.abs() >= 1.0 {
        format!("{price:.2}")
    } else {
        let formatted = format!("{price:.10}");

        formatted
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeta_test_support::settings_tests;
    use crate::config::HttpConfig;
    use model::FiatQuote;

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
            client: client::Client::new("test-api-key", &HttpConfig::default()).unwrap(),
            coins: TtlCache::with_value(CoinCache::from(test_coins()), CACHE_TTL),
            fiat: TtlCache::new(CACHE_TTL),
            default_currency: DEFAULT_CURRENCY.to_string(),
            max_name_typos: 2,
        }
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

    /// Builds a quote for `Bitcoin (BTC)` in USD.
    fn test_quote() -> QuoteData {
        QuoteData {
            name: "Bitcoin".to_string(),
            symbol: "BTC".to_string(),
            quote: HashMap::from([(
                "USD".to_string(),
                FiatQuote {
                    price: Some(97231.504),
                    percent_change_1h: Some(0.5),
                    percent_change_24h: Some(-1.2),
                    percent_change_7d: Some(10.25),
                },
            )]),
        }
    }

    #[test]
    fn commands_are_consistent() {
        assert!(COMMANDS.contains(&CC));

        for (spec, _) in COIN_COMMANDS {
            assert!(
                COMMANDS.contains(spec),
                "{spec:?} missing from COMMANDS"
            );
        }

        let mut commands = COMMANDS
            .iter()
            .map(CommandSpec::trigger)
            .collect::<Vec<_>>();
        commands.sort_unstable();
        commands.dedup();
        assert_eq!(
            commands.len(),
            COMMANDS.len(),
            "duplicate command in COMMANDS"
        );
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

        let coin = plugin.resolve_coin("btc").expect("symbol lookup");
        assert_eq!(coin.name, "Bitcoin");
        assert_eq!(coin.id, 1);

        // `bitcoin` matches `Bitcoin` rather than the longer `Bitcoin Cash`.
        let coin = plugin.resolve_coin("bitcoin").expect("name lookup");
        assert_eq!(coin.name, "Bitcoin");

        // Misspelled names still resolve, including with substituted characters.
        let coin = plugin.resolve_coin("bitcon").expect("fuzzy lookup");
        assert_eq!(coin.name, "Bitcoin");

        let coin = plugin.resolve_coin("bitkoin").expect("typo lookup");
        assert_eq!(coin.name, "Bitcoin");

        let coin = plugin.resolve_coin("doge").expect("fuzzy lookup");
        assert_eq!(coin.name, "Dogecoin");

        assert!(plugin.resolve_coin("").is_none());
        assert!(plugin.resolve_coin("   ").is_none());
        assert!(plugin.resolve_coin("zzzz").is_none());
    }

    #[test]
    fn fuzzy_matching_allows_missing_and_substituted_characters() {
        assert!(matches_within("bitcoin", "bitcoin", 0));
        assert!(matches_within("bitcon", "bitcoin", 0));
        assert!(matches_within("bitkoin", "bitcoin", 1));
        assert!(!matches_within("bitkoin", "bitcoin", 0));
        assert!(!matches_within("dogecoin", "doge", 0));
    }

    /// Builds a coin map keyed by symbol for name matching tests.
    fn matching_coins() -> HashMap<String, Coin> {
        [
            (1, "Bitcoin", "BTC"),
            (2, "Bitcoin Cash", "BCH"),
            (3, "PAX Gold", "PAXG"),
            (4, "Golem", "GLM"),
            (5, "Uniswap", "UNI"),
            (6, "SushiSwap", "SUSHI"),
            (7, "The Graph", "GRT"),
            (8, "Shiba Inu", "SHIB"),
        ]
        .into_iter()
        .map(|(id, name, symbol)| {
            (
                symbol.to_string(),
                Coin {
                    id,
                    name: name.to_string(),
                    symbol: symbol.to_string(),
                },
            )
        })
        .collect()
    }

    #[test]
    fn prefers_names_containing_the_query() {
        let coins = matching_coins();
        let symbol = |query: &str| find_by_name(&coins, query, 2).map(|coin| coin.symbol);

        assert_eq!(symbol("bitcoin").as_deref(), Some("BTC"));
        assert_eq!(symbol("bitkoin").as_deref(), Some("BTC"));
        assert_eq!(symbol("gold").as_deref(), Some("PAXG"));
        assert_eq!(symbol("swap").as_deref(), Some("UNI"));
        assert_eq!(symbol("graph").as_deref(), Some("GRT"));
        assert_eq!(symbol("inu").as_deref(), Some("SHIB"));
    }

    #[test]
    fn parses_currencies() {
        let is_valid = |currency: &str| matches!(currency, "USD" | "EUR" | "JPY");

        assert_eq!(parse_currency(None, "USD", is_valid).unwrap(), "USD");
        assert_eq!(parse_currency(Some("eur"), "USD", is_valid).unwrap(), "EUR");
        assert_eq!(parse_currency(Some("JPY"), "USD", is_valid).unwrap(), "JPY");
        assert!(matches!(
            parse_currency(Some("xyz"), "USD", is_valid),
            Err(Error::InvalidCurrency(currency)) if currency == "XYZ"
        ));
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

    #[test]
    fn parses_currencies_without_fiat_cache() {
        // Without a fiat cache, validation is deferred to the API.
        assert_eq!(parse_currency(Some("xyz"), "USD", |_| true).unwrap(), "XYZ");
    }

    #[test]
    fn sanitizes_currencies() {
        // Control characters (which include all IRC formatting bytes) must not survive into
        // the currency that is echoed back on error.
        let is_valid = |currency: &str| currency == "USD";

        assert!(matches!(
            parse_currency(Some("\u{2}xyz\u{f}"), "USD", is_valid),
            Err(Error::InvalidCurrency(currency)) if currency == "XYZ"
        ));
    }

    #[test]
    fn parses_command_options() {
        let opts: QuoteOpts = BTC.parse_args("").unwrap();
        assert_eq!(opts.currency, None);

        let opts: QuoteOpts = BTC.parse_args("eur").unwrap();
        assert_eq!(opts.currency.as_deref(), Some("eur"));

        let opts: CoinOpts = CC.parse_args("bitcoin eur").unwrap();
        assert_eq!(opts.coin, "bitcoin");
        assert_eq!(opts.currency.as_deref(), Some("eur"));

        // `.cc` requires a coin.
        assert!(matches!(
            CC.parse_args::<CoinOpts>(""),
            Err(ArgsError::Usage(_))
        ));
    }

    #[test]
    fn formats_quote() {
        let formatted = format_quote(&test_quote(), "USD", Some("$"));

        assert_eq!(
            formatted,
            "\x0310> Bitcoin (\x0fBTC\x0310) is currently trading at\x0f $97231.50\x0310 \
             (1 Hour Change:\x0f \x033+0.50%\x0f\x0310 24 Hour Change:\x0f \
             \x034-1.20%\x0f\x0310 7 Day Change:\x0f \x033+10.25%\x0f\x0310)"
        );
    }

    #[test]
    fn formats_quote_with_unknown_currency_code() {
        let mut quote = test_quote();
        quote.quote = HashMap::from([(
            "AUD".to_string(),
            FiatQuote {
                price: Some(1234.5),
                percent_change_1h: None,
                percent_change_24h: None,
                percent_change_7d: None,
            },
        )]);

        let formatted = format_quote(&quote, "AUD", None);

        assert!(formatted.contains("1234.50 AUD"));
        assert!(formatted.contains("n/a"));
    }

    #[test]
    fn formats_quote_for_missing_currency() {
        let formatted = format_quote(&test_quote(), "DKK", None);

        assert!(formatted.contains("has no quote in DKK"), "{formatted:?}");
    }

    #[test]
    fn formats_prices() {
        assert_eq!(format_price(97231.504), "97231.50");
        assert_eq!(format_price(1.0), "1.00");
        assert_eq!(format_price(0.5), "0.5");
        assert_eq!(format_price(0.000_012_34), "0.00001234");
        assert_eq!(format_price(0.0), "0");
    }

    #[test]
    fn formats_money() {
        assert_eq!(format_money(97231.504, "USD", Some("$")), "$97231.50");
        assert_eq!(format_money(1234.5, "EUR", Some("€")), "€1234.50");
        assert_eq!(format_money(1234.5, "AUD", None), "1234.50 AUD");
    }

    #[test]
    fn formats_changes() {
        assert_eq!(format_change(Some(0.5)), "\x033+0.50%\x0f");
        assert_eq!(format_change(Some(-1.2)), "\x034-1.20%\x0f");
        assert_eq!(format_change(Some(0.0)), "\x033+0.00%\x0f");
        assert_eq!(format_change(None), "\x0fn/a");
    }
}
