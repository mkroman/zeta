//! Crypto currency quotes via the CoinMarketCap API.
//!
//! Provides the `.cc` command for quoting an arbitrary coin by symbol or (fuzzy) name, as well
//! as fixed triggers such as `.btc` and `.eth` that quote a known symbol. Both accept an
//! optional fiat currency to convert the price into (e.g. `.btc eur`).
//!
//! Name lookups are served from a cache of the top 250 cryptocurrencies by market cap, fetched
//! once when the bot connects and not refreshed afterwards. Symbols of any coin resolve
//! regardless, but coins listed after startup do not resolve by name.

use std::collections::HashMap;
use std::sync::RwLock;

use argh::FromArgs;
use frizbee::{CaseMatching, Config, Matcher, Matching};
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tracing::{debug, warn};

use crate::{http, plugin::prelude::*};

/// Base URL of the CoinMarketCap Pro API.
pub const API_BASE_URL: &str = "https://pro-api.coinmarketcap.com";

/// The HTTP header that carries the CoinMarketCap API key on requests.
const API_KEY_HEADER: HeaderName = HeaderName::from_static("x-cmc_pro_api_key");

/// The number of top cryptocurrencies (by market cap) cached for symbol and name lookups.
const LISTINGS_LIMIT: u32 = 250;

/// The default fiat currency that coin prices are quoted in.
pub const DEFAULT_CURRENCY: &str = "USD";

/// The fiat currencies supported for price conversion, as listed by the CoinMarketCap API
/// documentation.
pub const VALID_CURRENCIES: &[&str] = &[
    "AUD", "BRL", "CAD", "CHF", "CLP", "CNY", "CZK", "DKK", "EUR", "GBP", "HKD", "HUF", "IDR",
    "ILS", "INR", "JPY", "KRW", "MXN", "MYR", "NOK", "NZD", "PHP", "PKR", "PLN", "RUB", "SEK",
    "SGD", "THB", "TRY", "TWD", "USD", "ZAR",
];

/// The maximum number of typos (needle characters missing from the name) allowed when fuzzy
/// matching a coin name.
const MAX_NAME_TYPOS: u16 = 2;

/// The `.cc` command for quoting an arbitrary coin by symbol or name.
const CC: Prefix = Prefix::new(".cc");

/// The `.btc` command.
const BTC: Prefix = Prefix::new(".btc");
/// The `.eth` command.
const ETH: Prefix = Prefix::new(".eth");
/// The `.zcash` command.
const ZCASH: Prefix = Prefix::new(".zcash");
/// The `.zec` command.
const ZEC: Prefix = Prefix::new(".zec");
/// The `.ans` command (Antshares, the former name of Neo).
const ANS: Prefix = Prefix::new(".ans");
/// The `.neo` command.
const NEO: Prefix = Prefix::new(".neo");
/// The `.stellar` command.
const STELLAR: Prefix = Prefix::new(".stellar");
/// The `.xmr` command.
const XMR: Prefix = Prefix::new(".xmr");
/// The `.xrp` command.
const XRP: Prefix = Prefix::new(".xrp");
/// The `.ltc` command.
const LTC: Prefix = Prefix::new(".ltc");
/// The `.etc` command.
const ETC: Prefix = Prefix::new(".etc");
/// The `.golem` command.
const GOLEM: Prefix = Prefix::new(".golem");
/// The `.sia` command.
const SIA: Prefix = Prefix::new(".sia");
/// The `.doge` command.
const DOGE: Prefix = Prefix::new(".doge");
/// The `.maid` command.
const MAID: Prefix = Prefix::new(".maid");
/// The `.bcash` command.
const BCASH: Prefix = Prefix::new(".bcash");
/// The `.trump` command.
const TRUMP: Prefix = Prefix::new(".trump");

/// Fixed coin commands, mapped to the symbol they quote.
const COIN_COMMANDS: &[(Prefix, &str)] = &[
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

/// All command triggers handled by this plugin.
const COMMANDS: &[Prefix] = &[
    CC, BTC, ETH, ZCASH, ZEC, ANS, NEO, STELLAR, XMR, XRP, LTC, ETC, GOLEM, SIA, DOGE, MAID,
    BCASH, TRUMP,
];

/// Command options for the fixed coin commands.
#[derive(FromArgs, Debug)]
struct QuoteOpts {
    /// fiat currency to convert the price into (defaults to USD)
    #[argh(positional)]
    currency: Option<String>,
}

/// The usage hint for the `.cc` command.
const CC_USAGE: &str = "Usage: .cc \x0f<symbol> [currency]";

/// Command options for the `.cc` command.
#[derive(FromArgs, Debug)]
struct CoinOpts {
    /// the coin to quote, by symbol or name
    #[argh(positional)]
    coin: String,
    /// fiat currency to convert the price into (defaults to USD)
    #[argh(positional)]
    currency: Option<String>,
}

/// Errors that can occur during coin lookups.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The HTTP request failed.
    #[error("request error")]
    Request(#[source] reqwest::Error),
    /// The response could not be parsed.
    #[error("could not parse response")]
    Deserialize(#[source] serde_path_to_error::Error<serde_json::Error>),
    /// The API returned an error status (e.g. an invalid symbol).
    #[error("{0}")]
    Api(String),
    /// An unsupported conversion currency was requested.
    #[error("Invalid currency: {0}")]
    InvalidCurrency(String),
    /// The requested coin was not found.
    #[error("not found")]
    NotFound,
}

/// A coin reference from the CoinMarketCap listings.
#[derive(Clone, Debug, Deserialize)]
struct CoinRef {
    /// The CoinMarketCap id of the coin.
    id: u64,
    /// The display name of the coin (e.g. `Bitcoin`).
    name: String,
    /// The ticker symbol of the coin (e.g. `BTC`).
    symbol: String,
}

/// Identifies the coin to quote, either by CoinMarketCap id or by symbol.
#[derive(Debug)]
enum CoinQuery {
    /// Quote by CoinMarketCap id.
    Id(u64),
    /// Quote by ticker symbol.
    Symbol(String),
}

/// A coin quote from the `quotes/latest` endpoint.
#[derive(Debug, Deserialize)]
struct QuoteData {
    /// The display name of the coin.
    name: String,
    /// The ticker symbol of the coin.
    symbol: String,
    /// Quotes keyed by conversion currency.
    quote: HashMap<String, FiatQuote>,
}

/// A quote in a single conversion currency.
#[derive(Debug, Deserialize)]
struct FiatQuote {
    /// The current price.
    price: Option<f64>,
    /// The price change over the last hour, in percent.
    percent_change_1h: Option<f64>,
    /// The price change over the last 24 hours, in percent.
    percent_change_24h: Option<f64>,
    /// The price change over the last 7 days, in percent.
    percent_change_7d: Option<f64>,
}

/// An envelope for CoinMarketCap API responses.
#[derive(Debug, Deserialize)]
struct CmcEnvelope<T> {
    /// The data payload, present on success.
    data: Option<T>,
    /// The status of the request.
    status: CmcStatus,
}

/// The status of a CoinMarketCap API response.
#[derive(Debug, Deserialize)]
struct CmcStatus {
    /// The error code; `0` indicates success.
    error_code: i32,
    /// A human-readable description of the error, if any.
    error_message: Option<String>,
}

/// Crypto currency quotes plugin, backed by the CoinMarketCap API.
pub struct CryptoCoins {
    /// HTTP client for CoinMarketCap API requests, with the API key set as a default header.
    client: reqwest::Client,
    /// Cached references of the top 250 cryptocurrencies keyed by symbol, populated on
    /// connect.
    coins: RwLock<HashMap<String, CoinRef>>,
}

#[async_trait]
impl Plugin<Context> for CryptoCoins {
    fn new(_ctx: &Context) -> Result<Self, ZetaError> {
        let api_key = require_env("COINMARKETCAP_API_KEY")?;
        let client = build_client(&api_key)?;

        Ok(Self {
            client,
            coins: RwLock::new(HashMap::new()),
        })
    }

    fn metadata() -> Metadata {
        Metadata {
            name: "cryptocoins".into(),
            authors: vec!["Mikkel Kroman <mk@maero.dk>".into()],
        }
    }

    fn commands(&self) -> &'static [Prefix] {
        COMMANDS
    }

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        match self.cache_coins().await {
            Ok(count) => debug!(count, "cached the top cryptocurrencies"),
            Err(err) => warn!(error = %err, "could not cache the top cryptocurrencies"),
        }

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
        if *command == CC {
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

impl CryptoCoins {
    /// Handles a `.cc` invocation, resolving `args` to a coin by symbol or fuzzy name.
    async fn handle_cc(
        &self,
        client: &Client,
        channel: &str,
        args: &str,
    ) -> Result<(), ZetaError> {
        let opts = match CC.parse_args::<CoinOpts>(args) {
            Ok(opts) if !opts.coin.trim().is_empty() => opts,
            _ => {
                client.send_privmsg(channel, formatted(CC_USAGE))?;
                return Ok(());
            }
        };

        let currency = match parse_currency(opts.currency.as_deref()) {
            Ok(currency) => currency,
            Err(err) => return Self::reply_error(client, channel, &err),
        };

        let query = opts.coin;

        // Prefer a cached coin (by symbol, then by fuzzy name); fall back to letting the API
        // resolve the query as a symbol (e.g. when the cache has not been populated yet, or the
        // coin is outside the cached top 250).
        let quote = match self.resolve_coin(&query) {
            Some(coin) => self.get_quote(CoinQuery::Id(coin.id), &currency).await,
            None => {
                self.get_quote(CoinQuery::Symbol(query.to_ascii_uppercase()), &currency)
                    .await
            }
        };

        Self::reply_quote(client, channel, quote, &currency)
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

        let currency = match parse_currency(opts.currency.as_deref()) {
            Ok(currency) => currency,
            Err(err) => return Self::reply_error(client, channel, &err),
        };

        let quote = self
            .get_quote(CoinQuery::Symbol(symbol.to_string()), &currency)
            .await;

        Self::reply_quote(client, channel, quote, &currency)
    }

    /// Replies with the result of a coin quote lookup.
    fn reply_quote(
        client: &Client,
        channel: &str,
        quote: Result<QuoteData, Error>,
        currency: &str,
    ) -> Result<(), ZetaError> {
        match quote {
            Ok(quote) => client.send_privmsg(channel, format_quote(&quote, currency))?,
            Err(err) => Self::reply_error(client, channel, &err)?,
        }

        Ok(())
    }

    /// Replies with a friendly error message, logging unexpected failures.
    fn reply_error(client: &Client, channel: &str, err: &Error) -> Result<(), ZetaError> {
        // Unknown coins and unsupported currencies are user errors: they are replied to
        // verbatim, without logging. Anything else is unexpected and worth a warning.
        if !matches!(err, Error::NotFound | Error::InvalidCurrency(_)) {
            warn!(error = %err, "coin lookup failed");
        }

        let message = match err {
            Error::NotFound => "No such coin".to_string(),
            Error::InvalidCurrency(_) => err.to_string(),
            Error::Api(message) => format!("Could not retrieve coin information: {message}"),
            Error::Request(_) => "Could not reach the CoinMarketCap API".to_string(),
            Error::Deserialize(_) => "Could not parse the CoinMarketCap response".to_string(),
        };

        client.send_privmsg(channel, formatted(&message))?;

        Ok(())
    }

    /// Resolves a user query to a coin, by exact symbol match first and by fuzzy name match
    /// second.
    fn resolve_coin(&self, query: &str) -> Option<CoinRef> {
        let coins = self
            .coins
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        coins
            .get(&query.to_ascii_uppercase())
            .cloned()
            .or_else(|| find_by_name(&coins, query))
    }

    /// Fetches the CoinMarketCap listings of the top cryptocurrencies and caches them for
    /// symbol and name lookups.
    ///
    /// # Returns
    ///
    /// The number of coins cached.
    async fn cache_coins(&self) -> Result<usize, Error> {
        let coins = self.listings().await?;
        let count = coins.len();

        {
            let mut cache = self
                .coins
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            *cache = coins
                .into_iter()
                .map(|coin| (coin.symbol.clone(), coin))
                .collect();
        }

        Ok(count)
    }

    /// Fetches the top cryptocurrencies (by market cap) from the CoinMarketCap API.
    async fn listings(&self) -> Result<Vec<CoinRef>, Error> {
        debug!(limit = LISTINGS_LIMIT, "fetching the top cryptocurrencies");

        let response = self
            .client
            .get(format!(
                "{API_BASE_URL}/v3/cryptocurrency/listings/latest"
            ))
            .query(&[("limit", LISTINGS_LIMIT.to_string())])
            .send()
            .await
            .map_err(Error::Request)?;

        let status = response.status();
        let text = response.text().await.map_err(Error::Request)?;

        if !status.is_success() {
            return Err(api_error(status, &text));
        }

        let parsed: CmcEnvelope<Vec<CoinRef>> = decode_response(&text)?;

        Ok(parsed.data.unwrap_or_default())
    }

    /// Fetches the quote for a single coin in the given conversion currency.
    async fn get_quote(&self, coin: CoinQuery, convert: &str) -> Result<QuoteData, Error> {
        let mut query: Vec<(&str, String)> = vec![("convert", convert.to_string())];

        match coin {
            CoinQuery::Id(id) => query.push(("id", id.to_string())),
            CoinQuery::Symbol(symbol) => query.push(("symbol", symbol)),
        }

        debug!(?query, "requesting coin quote");

        let response = self
            .client
            .get(format!("{API_BASE_URL}/v1/cryptocurrency/quotes/latest"))
            .query(&query)
            .send()
            .await
            .map_err(Error::Request)?;

        let status = response.status();
        let text = response.text().await.map_err(Error::Request)?;

        if !status.is_success() {
            return Err(api_error(status, &text));
        }

        let parsed: CmcEnvelope<HashMap<String, QuoteData>> = decode_response(&text)?;

        // The data payload is keyed by the requested identifier (symbol or id); a single coin is
        // requested, so the first (and only) value is the quote.
        parsed
            .data
            .and_then(|data| data.into_values().next())
            .ok_or(Error::NotFound)
    }
}

/// Parses and validates the optional conversion currency of a command invocation.
///
/// The input is sanitized before validation, so an unsupported currency can be echoed back to
/// the channel without leaking IRC formatting control characters.
fn parse_currency(currency: Option<&str>) -> Result<String, Error> {
    let currency = sanitize(currency.unwrap_or(DEFAULT_CURRENCY)).to_ascii_uppercase();

    if VALID_CURRENCIES.contains(&currency.as_str()) {
        Ok(currency)
    } else {
        Err(Error::InvalidCurrency(currency))
    }
}

/// Builds the HTTP client used for CoinMarketCap API requests, with the API key and the
/// expected response type set as default headers.
///
/// # Errors
///
/// Returns [`ZetaError`] if the API key is not a valid HTTP header value, or the client could
/// not be built.
fn build_client(api_key: &str) -> Result<reqwest::Client, ZetaError> {
    let headers = HeaderMap::from_iter([
        (ACCEPT, HeaderValue::from_static("application/json")),
        (
            API_KEY_HEADER,
            HeaderValue::from_str(api_key).map_err(plugin_err)?,
        ),
    ]);

    http::client::builder()
        .default_headers(headers)
        .build()
        .map_err(plugin_err)
}

/// Finds the best fuzzy name match for `query` among `coins`, if any.
///
/// Candidates are scored with frizbee's Smith-Waterman fuzzy matcher (case-insensitive, with
/// typo resistance); the highest score wins, preferring the shorter name on ties (e.g.
/// `Bitcoin` over `Bitcoin Cash` for the query `bitcoin`).
fn find_by_name(coins: &HashMap<String, CoinRef>, query: &str) -> Option<CoinRef> {
    if query.trim().is_empty() {
        return None;
    }

    let candidates: Vec<&CoinRef> = coins.values().collect();
    let names: Vec<&str> = candidates.iter().map(|coin| coin.name.as_str()).collect();

    let config = Config::default()
        .matching(Matching::Fuzzy)
        .casing(CaseMatching::Ignore)
        .max_typos(Some(MAX_NAME_TYPOS));
    let mut matches = Matcher::new(query, &config).match_list(&names);

    matches.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| {
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

/// Builds an API error from a non-success response, preserving the API error message if present.
///
/// Bad requests (e.g. an unknown symbol or id) are reported as a miss rather than an error.
fn api_error(status: StatusCode, body: &str) -> Error {
    let message = serde_json::from_str::<CmcEnvelope<serde_json::Value>>(body)
        .ok()
        .filter(|envelope| envelope.status.error_code != 0)
        .and_then(|envelope| envelope.status.error_message);

    if status == StatusCode::BAD_REQUEST {
        debug!(?message, "the api rejected the request");

        return Error::NotFound;
    }

    message.map_or_else(
        || Error::Api(format!("unexpected response status {status}")),
        Error::Api,
    )
}

/// Deserializes a response body into `T`, logging parse failures.
fn decode_response<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
    let deserializer = &mut serde_json::Deserializer::from_str(text);

    serde_path_to_error::deserialize(deserializer)
        .inspect_err(|err| debug!(?err, %text, "could not deserialize response"))
        .map_err(Error::Deserialize)
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
/// cryptocoins script.
fn formatted(s: &str) -> String {
    format!("\x0310> {s}")
}

/// Formats a coin quote as an IRC message, e.g.:
///
/// `\x0310> Bitcoin (\x0fBTC\x0310) is currently trading at\x03 $97231.50\x0310 (...)`
fn format_quote(quote: &QuoteData, currency: &str) -> String {
    let name = &quote.name;
    let symbol = &quote.symbol;

    let Some(fiat) = quote.quote.get(currency) else {
        return formatted(&format!("{name} (\x0f{symbol}\x0310) has no quote in {currency}"));
    };

    let price = fiat
        .price
        .map_or_else(|| "n/a".to_string(), |price| format_money(price, currency));

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

/// Formats a price with a currency symbol when one is known, appending the currency code
/// otherwise.
fn format_money(price: f64, currency: &str) -> String {
    let price = format_price(price);

    currency_symbol(currency).map_or_else(
        || format!("{price} {currency}"),
        |symbol| format!("{symbol}{price}"),
    )
}

/// Returns the currency symbol for a well-known fiat currency, if any.
fn currency_symbol(currency: &str) -> Option<&'static str> {
    match currency {
        "USD" => Some("$"),
        "EUR" => Some("€"),
        "GBP" => Some("£"),
        "JPY" | "CNY" => Some("¥"),
        _ => None,
    }
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

    /// Builds a coin cache for lookup tests.
    fn test_coins() -> HashMap<String, CoinRef> {
        [
            CoinRef {
                id: 1,
                name: "Bitcoin".to_string(),
                symbol: "BTC".to_string(),
            },
            CoinRef {
                id: 2,
                name: "Bitcoin Cash".to_string(),
                symbol: "BCH".to_string(),
            },
            CoinRef {
                id: 1027,
                name: "Ethereum".to_string(),
                symbol: "ETH".to_string(),
            },
            CoinRef {
                id: 74,
                name: "Dogecoin".to_string(),
                symbol: "DOGE".to_string(),
            },
        ]
        .into_iter()
        .map(|coin| (coin.symbol.clone(), coin))
        .collect()
    }

    /// Builds a plugin instance for lookup tests, without touching the API.
    fn test_plugin() -> CryptoCoins {
        CryptoCoins {
            client: http::build_client(),
            coins: RwLock::new(test_coins()),
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

        for (prefix, _) in COIN_COMMANDS {
            assert!(COMMANDS.contains(prefix), "{prefix:?} missing from COMMANDS");
        }

        let mut commands = COMMANDS.iter().map(Prefix::as_str).collect::<Vec<_>>();
        commands.sort_unstable();
        commands.dedup();
        assert_eq!(commands.len(), COMMANDS.len(), "duplicate command in COMMANDS");
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
        assert_eq!(parse_currency(None).unwrap(), "USD");
        assert_eq!(parse_currency(Some("eur")).unwrap(), "EUR");
        assert_eq!(parse_currency(Some("JPY")).unwrap(), "JPY");
        assert!(matches!(
            parse_currency(Some("xyz")),
            Err(Error::InvalidCurrency(currency)) if currency == "XYZ"
        ));
    }

    #[test]
    fn sanitizes_currencies() {
        // Control characters (which include all IRC formatting bytes) must not survive into
        // the currency that is echoed back on error.
        assert!(matches!(
            parse_currency(Some("\u{2}xyz\u{f}")),
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
        let formatted = format_quote(&test_quote(), "USD");

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

        let formatted = format_quote(&quote, "AUD");

        assert!(formatted.contains("1234.50 AUD"));
        assert!(formatted.contains("n/a"));
    }

    #[test]
    fn formats_quote_for_missing_currency() {
        let formatted = format_quote(&test_quote(), "DKK");

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
        assert_eq!(format_money(97231.504, "USD"), "$97231.50");
        assert_eq!(format_money(1234.5, "EUR"), "€1234.50");
        assert_eq!(format_money(1234.5, "AUD"), "1234.50 AUD");
    }

    #[test]
    fn formats_changes() {
        assert_eq!(format_change(Some(0.5)), "\x033+0.50%\x0f");
        assert_eq!(format_change(Some(-1.2)), "\x034-1.20%\x0f");
        assert_eq!(format_change(Some(0.0)), "\x033+0.00%\x0f");
        assert_eq!(format_change(None), "\x0fn/a");
    }

    #[test]
    fn decodes_quote_response() {
        let text = r#"{"data":{"BTC":{"id":1,"name":"Bitcoin","symbol":"BTC","slug":"bitcoin","quote":{"USD":{"price":97231.504,"volume_24h":48000000000,"percent_change_1h":0.5,"percent_change_24h":-1.2,"percent_change_7d":10.25,"last_updated":"2026-09-09T00:00:00.000Z"}}}},"status":{"timestamp":"2026-09-09T00:00:00.000Z","error_code":0,"error_message":null,"elapsed":12,"credit_count":1}}"#;

        let parsed: CmcEnvelope<HashMap<String, QuoteData>> = decode_response(text).unwrap();
        let quote = parsed.data.unwrap().remove("BTC").unwrap();

        assert_eq!(quote.name, "Bitcoin");
        assert_eq!(quote.symbol, "BTC");
        assert_eq!(quote.quote["USD"].price, Some(97231.504));
        assert_eq!(quote.quote["USD"].percent_change_24h, Some(-1.2));
    }

    #[test]
    fn decodes_listings_response() {
        let text = r#"{"data":[{"id":1,"name":"Bitcoin","symbol":"BTC","slug":"bitcoin","cmc_rank":1,"tags":["mineable"],"quote":[{"symbol":"USD","price":63120.95,"percent_change_1h":0.61,"percent_change_24h":-1.19,"percent_change_7d":-0.28}]},{"id":1027,"name":"Ethereum","symbol":"ETH","slug":"ethereum","cmc_rank":2}],"status":{"timestamp":"2026-09-09T00:00:00.000Z","error_code":0,"error_message":null,"elapsed":12,"credit_count":1}}"#;

        let parsed: CmcEnvelope<Vec<CoinRef>> = decode_response(text).unwrap();
        let coins = parsed.data.unwrap();

        assert_eq!(coins.len(), 2);
        assert_eq!(coins[0].symbol, "BTC");
        assert_eq!(coins[1].id, 1027);
    }

    #[test]
    fn maps_bad_request_to_not_found() {
        let text = r#"{"status":{"timestamp":"2026-09-09T00:00:00.000Z","error_code":400,"error_message":"Invalid value for \"symbol\"","elapsed":5,"credit_count":1}}"#;

        let err = api_error(StatusCode::BAD_REQUEST, text);

        assert!(matches!(err, Error::NotFound));
    }

    #[test]
    fn maps_bad_request_with_unparseable_body_to_not_found() {
        let err = api_error(StatusCode::BAD_REQUEST, "<html>gateway error</html>");

        assert!(matches!(err, Error::NotFound));
    }

    #[test]
    fn builds_api_error_from_error_payload() {
        let text = r#"{"status":{"timestamp":"2026-09-09T00:00:00.000Z","error_code":1001,"error_message":"This API Key is invalid.","elapsed":5,"credit_count":1}}"#;

        let err = api_error(StatusCode::UNAUTHORIZED, text);

        assert_eq!(err.to_string(), "This API Key is invalid.");
    }

    #[test]
    fn builds_api_error_from_unparseable_payload() {
        let err = api_error(StatusCode::INTERNAL_SERVER_ERROR, "<html>oops</html>");

        assert!(err.to_string().contains("500"), "{err:?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires COINMARKETCAP_API_KEY and network access"]
    async fn live_quote_by_symbol() {
        let Ok(api_key) = std::env::var("COINMARKETCAP_API_KEY") else {
            return;
        };

        let plugin = CryptoCoins {
            client: build_client(&api_key).unwrap(),
            coins: RwLock::new(HashMap::new()),
        };

        let quote = plugin
            .get_quote(CoinQuery::Symbol("BTC".to_string()), "USD")
            .await
            .unwrap();

        assert_eq!(quote.symbol, "BTC");
        println!("{}", format_quote(&quote, "USD"));
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "requires COINMARKETCAP_API_KEY and network access"]
    async fn live_cache_coins() {
        let Ok(api_key) = std::env::var("COINMARKETCAP_API_KEY") else {
            return;
        };

        let plugin = CryptoCoins {
            client: build_client(&api_key).unwrap(),
            coins: RwLock::new(HashMap::new()),
        };

        let count = plugin.cache_coins().await.unwrap();

        assert_eq!(count, usize::try_from(LISTINGS_LIMIT).unwrap());
        assert!(plugin.resolve_coin("bitcoin").is_some());
    }
}
