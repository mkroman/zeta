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

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{PoisonError, RwLock};
use std::time::{Duration, Instant};

use argh::{ArgsInfo, FromArgs};
use frizbee::{CaseMatching, Config, Matcher, Matching};
use tracing::{debug, warn};

use crate::plugin::prelude::*;

mod client;
mod error;
mod model;

use error::Error;
use model::{Coin, CoinQuery, DEFAULT_CURRENCY, Fiat, QuoteData};

/// The maximum number of typos (needle characters missing from the name) allowed when fuzzy
/// matching a coin name.
const MAX_NAME_TYPOS: u16 = 2;

/// The `.cc` command.
const CC: PluginCommand = PluginCommand::with_args::<CoinOpts>(
    Prefix::new(".cc"),
    "Quote any coin by symbol or fuzzy name",
);

/// The `.btc` command.
const BTC: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".btc"),
    "Quote Bitcoin (BTC) in a fiat currency",
);
/// The `.eth` command.
const ETH: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".eth"),
    "Quote Ethereum (ETH) in a fiat currency",
);
/// The `.zcash` command.
const ZCASH: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".zcash"),
    "Quote Zcash (ZEC) in a fiat currency",
);
/// The `.zec` command.
const ZEC: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".zec"),
    "Quote Zcash (ZEC) in a fiat currency",
);
/// The `.ans` command (Antshares, the former name of Neo).
const ANS: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".ans"),
    "Quote Neo (NEO), formerly Antshares",
);
/// The `.neo` command.
const NEO: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".neo"),
    "Quote Neo (NEO) in a fiat currency",
);
/// The `.stellar` command.
const STELLAR: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".stellar"),
    "Quote Stellar (XLM) in a fiat currency",
);
/// The `.xmr` command.
const XMR: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".xmr"),
    "Quote Monero (XMR) in a fiat currency",
);
/// The `.xrp` command.
const XRP: PluginCommand =
    PluginCommand::with_args::<QuoteOpts>(Prefix::new(".xrp"), "Quote XRP in a fiat currency");
/// The `.ltc` command.
const LTC: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".ltc"),
    "Quote Litecoin (LTC) in a fiat currency",
);
/// The `.etc` command.
const ETC: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".etc"),
    "Quote Ethereum Classic (ETC) in a fiat currency",
);
/// The `.golem` command.
const GOLEM: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".golem"),
    "Quote Golem (GNT) in a fiat currency",
);
/// The `.sia` command.
const SIA: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".sia"),
    "Quote Siacoin (SC) in a fiat currency",
);
/// The `.doge` command.
const DOGE: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".doge"),
    "Quote Dogecoin (DOGE) in a fiat currency",
);
/// The `.maid` command.
const MAID: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".maid"),
    "Quote MaidSafeCoin (MAID) in a fiat currency",
);
/// The `.bcash` command.
const BCASH: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".bcash"),
    "Quote Bitcoin Cash (BCH) in a fiat currency",
);
/// The `.trump` command.
const TRUMP: PluginCommand = PluginCommand::with_args::<QuoteOpts>(
    Prefix::new(".trump"),
    "Quote Trump (TRUMP) in a fiat currency",
);

/// Fixed coin commands, mapped to the symbol they quote.
const COIN_COMMANDS: &[(Prefix, &str)] = &[
    (BTC.prefix(), "BTC"),
    (ETH.prefix(), "ETH"),
    (ZCASH.prefix(), "ZEC"),
    (ZEC.prefix(), "ZEC"),
    (ANS.prefix(), "NEO"),
    (NEO.prefix(), "NEO"),
    (STELLAR.prefix(), "XLM"),
    (XMR.prefix(), "XMR"),
    (XRP.prefix(), "XRP"),
    (LTC.prefix(), "LTC"),
    (ETC.prefix(), "ETC"),
    (GOLEM.prefix(), "GNT"),
    (SIA.prefix(), "SC"),
    (DOGE.prefix(), "DOGE"),
    (MAID.prefix(), "MAID"),
    (BCASH.prefix(), "BCH"),
    (TRUMP.prefix(), "TRUMP"),
];

/// The commands handled by this plugin.
const COMMANDS: &[PluginCommand] = &[
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

/// How long the cached coins and fiat currencies stay valid before being refreshed.
const CACHE_TTL: Duration = Duration::from_hours(24);

/// The top cryptocurrencies by market cap, keyed by ticker symbol.
///
/// Populated from the cryptocurrency map endpoint at startup and refreshed lazily after the
/// cache TTL expires.
#[derive(Debug, Default)]
struct CoinCache {
    /// The cached coins, keyed by ticker symbol.
    by_symbol: HashMap<String, Coin>,
    /// When the cache was last populated.
    fetched_at: Option<Instant>,
}

impl From<Vec<Coin>> for CoinCache {
    fn from(coins: Vec<Coin>) -> Self {
        // The map endpoint returns coins in ascending rank order; keep the first (highest
        // ranked) occurrence when multiple coins share a symbol.
        let mut by_symbol = HashMap::with_capacity(coins.len());

        for coin in coins {
            by_symbol.entry(coin.symbol.clone()).or_insert(coin);
        }

        Self {
            by_symbol,
            fetched_at: Some(Instant::now()),
        }
    }
}

/// The fiat currencies supported by the API for price conversion.
///
/// Populated from the fiat map endpoint at startup and refreshed lazily after the cache TTL
/// expires.
#[derive(Debug, Default)]
struct FiatCache {
    /// The valid fiat currency symbols (e.g. `USD`).
    symbols: HashSet<String>,
    /// The currency sign of each symbol (e.g. `USD` maps to `$`).
    signs: HashMap<String, String>,
    /// When the cache was last populated.
    fetched_at: Option<Instant>,
}

impl From<Vec<Fiat>> for FiatCache {
    fn from(fiats: Vec<Fiat>) -> Self {
        Self {
            symbols: fiats.iter().map(|fiat| fiat.symbol.clone()).collect(),
            signs: fiats
                .into_iter()
                .map(|fiat| (fiat.symbol, fiat.sign))
                .collect(),
            fetched_at: Some(Instant::now()),
        }
    }
}

/// A cache that expires after the cache TTL.
trait Cached {
    /// When the cache was last populated, if it was.
    fn fetched_at(&self) -> Option<Instant>;

    /// Returns whether the cache was populated within the cache TTL.
    fn is_fresh(&self) -> bool {
        self.fetched_at()
            .is_some_and(|fetched_at| fetched_at.elapsed() < CACHE_TTL)
    }
}

impl Cached for CoinCache {
    fn fetched_at(&self) -> Option<Instant> {
        self.fetched_at
    }
}

impl Cached for FiatCache {
    fn fetched_at(&self) -> Option<Instant> {
        self.fetched_at
    }
}

/// Crypto currency quotes plugin, backed by the CoinMarketCap API.
pub struct CoinMarketCap {
    /// Client for CoinMarketCap API requests, with the API key set as a default header.
    client: client::Client,
    /// The top cryptocurrencies by market cap, cached for 24 hours.
    coins: RwLock<CoinCache>,
    /// The fiat currencies supported for price conversion, cached for 24 hours.
    fiat: RwLock<FiatCache>,
}

#[async_trait]
impl Plugin<Context> for CoinMarketCap {
    fn new(ctx: &Context) -> Result<Self, ZetaError> {
        let api_key = require_env("COINMARKETCAP_API_KEY")?;
        let client = client::Client::new(&api_key, &ctx.config.http)?;

        Ok(Self {
            client,
            coins: RwLock::new(CoinCache::default()),
            fiat: RwLock::new(FiatCache::default()),
        })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "coinmarketcap".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn commands(&self) -> &'static [PluginCommand] {
        COMMANDS
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
        channel: &str,
        command: &Prefix,
        args: &str,
    ) -> Result<(), ZetaError> {
        if *command == CC.prefix() {
            return self.handle_cc(client, channel, args).await;
        }

        if let Some((_, symbol)) = COIN_COMMANDS.iter().find(|(prefix, _)| prefix == command) {
            return self
                .handle_coin(command, symbol, client, channel, args)
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
                client.send_privmsg(channel, formatted(CC_USAGE))?;
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
        command: &Prefix,
        symbol: &str,
        client: &Client,
        channel: &str,
        args: &str,
    ) -> Result<(), ZetaError> {
        let Ok(opts) = command.parse_args::<QuoteOpts>(args) else {
            client.send_privmsg(
                channel,
                formatted(&format!("Usage: {} \x0f[currency]", command.as_str())),
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

        let currency = match parse_currency(currency, |symbol| self.is_valid_currency(symbol)) {
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

        client.send_privmsg(channel, formatted(&err.to_string()))?;

        Ok(())
    }

    /// Resolves a user query to a coin, by exact symbol match first and by fuzzy name match
    /// second.
    fn resolve_coin(&self, query: &str) -> Option<Coin> {
        let coins = &self
            .coins
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .by_symbol;

        coins
            .get(&query.to_ascii_uppercase())
            .cloned()
            .or_else(|| find_by_name(coins, query))
    }

    /// Refreshes the coin cache when it is missing or older than the cache TTL.
    ///
    /// Failures are logged and leave the existing cache, if any, in place: names keep
    /// resolving with the stale coins, and an empty cache falls back to letting the API
    /// resolve queries as symbols.
    async fn ensure_coins_cached(&self) {
        refresh_cache(&self.coins, "the top cryptocurrencies", || {
            self.client.coin_map()
        })
        .await;
    }

    /// Refreshes the fiat currency cache when it is missing or older than the cache TTL.
    ///
    /// Failures are logged and leave the existing cache, if any, in place: quotes keep working
    /// with the stale currencies, and an empty cache defers currency validation to the API.
    async fn ensure_fiat_cached(&self) {
        refresh_cache(&self.fiat, "the supported fiat currencies", || {
            self.client.fiat_map()
        })
        .await;
    }

    /// Returns whether `currency` is a fiat currency supported for price conversion.
    ///
    /// Returns `true` while the fiat cache has not been populated, deferring validation to
    /// the API.
    fn is_valid_currency(&self, currency: &str) -> bool {
        let cache = self.fiat.read().unwrap_or_else(PoisonError::into_inner);

        cache.symbols.is_empty() || cache.symbols.contains(currency)
    }

    /// Returns the currency sign for `currency`, if known.
    fn fiat_sign(&self, currency: &str) -> Option<String> {
        self.fiat
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .signs
            .get(currency)
            .cloned()
    }
}

/// Refreshes the cache guarded by `lock` with the items fetched by `fetch` when the cache is
/// missing or older than the cache TTL.
///
/// Failures are logged and leave the existing cache, if any, in place.
async fn refresh_cache<C, T, E, F, Fut>(lock: &RwLock<C>, subject: &str, fetch: F)
where
    C: Cached + From<Vec<T>> + Send + Sync,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Vec<T>, E>> + Send,
    E: std::fmt::Display,
{
    let fresh = lock.read().unwrap_or_else(PoisonError::into_inner).is_fresh();

    if fresh {
        return;
    }

    match fetch().await {
        Ok(items) => {
            let count = items.len();

            *lock.write().unwrap_or_else(PoisonError::into_inner) = C::from(items);

            debug!(count, "cached {subject}");
        }
        Err(err) => warn!(error = %err, "could not cache {subject}"),
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
    is_valid: impl Fn(&str) -> bool,
) -> Result<String, Error> {
    let currency = sanitize(currency.unwrap_or(DEFAULT_CURRENCY)).to_ascii_uppercase();

    if is_valid(&currency) {
        Ok(currency)
    } else {
        Err(Error::InvalidCurrency(currency))
    }
}

/// Finds the best fuzzy name match for `query` among `coins`, if any.
///
/// Candidates are scored with frizbee's Smith-Waterman fuzzy matcher (case-insensitive, with
/// typo resistance); the highest score wins, preferring the shorter name on ties (e.g.
/// `Bitcoin` over `Bitcoin Cash` for the query `bitcoin`).
fn find_by_name(coins: &HashMap<String, Coin>, query: &str) -> Option<Coin> {
    if query.trim().is_empty() {
        return None;
    }

    let candidates: Vec<&Coin> = coins.values().collect();
    let names: Vec<&str> = candidates.iter().map(|coin| coin.name.as_str()).collect();

    let config = Config::default()
        .matching(Matching::Fuzzy)
        .casing(CaseMatching::Ignore)
        .max_typos(Some(MAX_NAME_TYPOS));
    let mut matches = Matcher::new(query, &config).match_list(&names);

    matches.sort_by(|a, b| {
        b.score.cmp(&a.score).then_with(|| {
            candidates[a.index as usize]
                .name
                .len()
                .cmp(&candidates[b.index as usize].name.len())
        })
    });

    matches
        .first()
        .map(|best| candidates[best.index as usize].clone())
}

/// Strips control characters from user-supplied input.
///
/// All IRC formatting bytes (color, bold, underline, reset, ...) are control characters, so
/// stripping them prevents user input from injecting formatting when echoed back to the
/// channel.
fn sanitize(input: &str) -> String {
    input.chars().filter(|c| !c.is_control()).collect()
}

/// Renders the given string as a message from the plugin, in the blue used by the original
/// coinmarketcap script.
fn formatted(s: &str) -> String {
    format!("\x0310> {s}")
}

/// Formats a coin quote as an IRC message, e.g.:
///
/// `\x0310> Bitcoin (\x0fBTC\x0310) is currently trading at\x03 $97231.50\x0310 (...)`
fn format_quote(quote: &QuoteData, currency: &str, sign: Option<&str>) -> String {
    let name = &quote.name;
    let symbol = &quote.symbol;

    let Some(fiat) = quote.quote.get(currency) else {
        return formatted(&format!(
            "{name} (\x0f{symbol}\x0310) has no quote in {currency}"
        ));
    };

    let price = fiat.price.map_or_else(
        || "n/a".to_string(),
        |price| format_money(price, currency, sign),
    );

    let change_hour = format_change(fiat.percent_change_1h);
    let change_day = format_change(fiat.percent_change_24h);
    let change_week = format_change(fiat.percent_change_7d);

    formatted(&format!(
        "{name} (\x0f{symbol}\x0310) is currently trading at\x03 {price}\x0310 (1 Hour Change:\
         \x0f {change_hour}\x0310 24 Hour Change:\x0f {change_day}\x0310 7 Day Change:\
         \x0f {change_week}\x0310)"
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
            coins: RwLock::new(CoinCache::from(test_coins())),
            fiat: RwLock::new(FiatCache::default()),
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
        assert!(
            COMMANDS
                .iter()
                .any(|command| command.prefix() == CC.prefix())
        );

        for (prefix, _) in COIN_COMMANDS {
            assert!(
                COMMANDS.iter().any(|command| command.prefix() == *prefix),
                "{prefix:?} missing from COMMANDS"
            );
        }

        let mut commands = COMMANDS
            .iter()
            .map(|command| command.prefix().as_str())
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
    fn parses_currencies() {
        let is_valid = |currency: &str| matches!(currency, "USD" | "EUR" | "JPY");

        assert_eq!(parse_currency(None, is_valid).unwrap(), "USD");
        assert_eq!(parse_currency(Some("eur"), is_valid).unwrap(), "EUR");
        assert_eq!(parse_currency(Some("JPY"), is_valid).unwrap(), "JPY");
        assert!(matches!(
            parse_currency(Some("xyz"), is_valid),
            Err(Error::InvalidCurrency(currency)) if currency == "XYZ"
        ));
    }

    #[test]
    fn validates_currencies_against_the_fiat_cache() {
        let mut plugin = test_plugin();

        // An empty fiat cache defers validation to the API.
        assert!(plugin.is_valid_currency("XYZ"));

        plugin.fiat = RwLock::new(FiatCache::from(vec![Fiat {
            sign: "$".to_string(),
            symbol: "USD".to_string(),
        }]));

        assert!(plugin.is_valid_currency("USD"));
        assert!(!plugin.is_valid_currency("XYZ"));
    }

    #[test]
    fn parses_currencies_without_fiat_cache() {
        // Without a fiat cache, validation is deferred to the API.
        assert_eq!(parse_currency(Some("xyz"), |_| true).unwrap(), "XYZ");
    }

    #[test]
    fn sanitizes_currencies() {
        // Control characters (which include all IRC formatting bytes) must not survive into
        // the currency that is echoed back on error.
        let is_valid = |currency: &str| currency == "USD";

        assert!(matches!(
            parse_currency(Some("\u{2}xyz\u{f}"), is_valid),
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
            "\x0310> Bitcoin (\x0fBTC\x0310) is currently trading at\x03 $97231.50\x0310 \
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
