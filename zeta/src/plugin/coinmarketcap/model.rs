//! The data model of the CoinMarketCap API responses.

use std::collections::HashMap;

use serde::Deserialize;

/// The default fiat currency that coin prices are quoted in.
pub const DEFAULT_CURRENCY: &str = "USD";

/// A coin from the CoinMarketCap cryptocurrency map.
///
/// The endpoint response also carries a rank, slug, activity state and historical ranges,
/// which the plugin does not use.
#[derive(Clone, Debug, Deserialize)]
pub struct Coin {
    /// The CoinMarketCap id of the coin.
    pub id: u64,
    /// The display name of the coin (e.g. `Bitcoin`).
    pub name: String,
    /// The ticker symbol of the coin (e.g. `BTC`).
    pub symbol: String,
}

/// A fiat currency from the CoinMarketCap fiat map.
///
/// The endpoint response also carries a CoinMarketCap id and a display name, which the plugin
/// does not use.
#[derive(Clone, Debug, Deserialize)]
pub struct Fiat {
    /// The currency sign (e.g. `$`).
    pub sign: String,
    /// The ISO symbol of the currency (e.g. `USD`).
    pub symbol: String,
}

/// Identifies the coin to quote, either by CoinMarketCap id or by symbol.
#[derive(Debug)]
pub enum CoinQuery {
    /// Quote by CoinMarketCap id.
    Id(u64),
    /// Quote by ticker symbol.
    Symbol(String),
}

/// A coin quote from the `quotes/latest` endpoint.
#[derive(Debug, Deserialize)]
pub struct QuoteData {
    /// The display name of the coin.
    pub name: String,
    /// The ticker symbol of the coin.
    pub symbol: String,
    /// Quotes keyed by conversion currency.
    pub quote: HashMap<String, FiatQuote>,
}

/// A coin from the `listings/latest` endpoint.
///
/// The endpoint response carries further auxiliary fields (`tags`, `platform`, supply
/// breakdowns), which the plugin does not use.
#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
pub struct Listing {
    /// The CoinMarketCap id of the coin.
    pub id: u64,
    /// The display name of the coin (e.g. `Ethereum`).
    pub name: String,
    /// The ticker symbol of the coin (e.g. `ETH`).
    pub symbol: String,
    /// The web-friendly shorthand of the coin name.
    pub slug: String,
    /// The coin's market cap rank.
    pub cmc_rank: Option<u64>,
    /// The number of active market pairs trading the coin.
    pub num_market_pairs: Option<u64>,
    /// The approximate number of coins circulating.
    pub circulating_supply: Option<f64>,
    /// The approximate total amount of coins in existence.
    pub total_supply: Option<f64>,
    /// The maximum amount of coins that will ever exist.
    pub max_supply: Option<f64>,
    /// Whether the coin's supply is known to be infinite.
    pub infinite_supply: Option<bool>,
    /// When the coin was added to CoinMarketCap.
    pub date_added: Option<String>,
    /// When the coin's market data was last updated.
    pub last_updated: Option<String>,
    /// Market quotes, one per conversion currency requested.
    pub quote: Vec<ListingQuote>,
}

/// A quote in a single conversion currency, as returned by the listings endpoint.
///
/// The endpoint response carries further volume and change fields, which the plugin does not
/// use.
#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
pub struct ListingQuote {
    /// The CoinMarketCap id of the conversion currency.
    pub id: u64,
    /// The ISO symbol of the conversion currency (e.g. `USD`).
    pub symbol: String,
    /// The current price in the conversion currency.
    pub price: Option<f64>,
}

/// A quote in a single conversion currency.
#[derive(Clone, Debug, Deserialize)]
pub struct FiatQuote {
    /// The current price.
    pub price: Option<f64>,
    /// The price change over the last hour, in percent.
    pub percent_change_1h: Option<f64>,
    /// The price change over the last 24 hours, in percent.
    pub percent_change_24h: Option<f64>,
    /// The price change over the last 7 days, in percent.
    pub percent_change_7d: Option<f64>,
}

/// An envelope for CoinMarketCap API responses.
#[derive(Debug, Deserialize)]
pub struct Envelope<T> {
    /// The data payload, present on success.
    pub data: Option<T>,
    /// The status of the request.
    pub status: Status,
}

/// The status of a CoinMarketCap API response.
#[derive(Debug, Deserialize)]
pub struct Status {
    /// The error code; `"0"` indicates success.
    ///
    /// The API returns it as a JSON string on the v3 endpoints (`"error_code":"0"`) and as an
    /// integer on the v1 ones (`"error_code":0`), so both shapes are accepted and normalized
    /// to a string.
    #[serde(deserialize_with = "deserialize_error_code")]
    pub error_code: String,
    /// A human-readable description of the error, if any.
    pub error_message: Option<String>,
}

/// Deserializes the response's error code, which the API returns as a JSON string on the v3
/// endpoints and as an integer on the v1 ones.
fn deserialize_error_code<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::String(code) => Ok(code),
        serde_json::Value::Number(code) => Ok(code.to_string()),
        _ => Err(serde::de::Error::custom(
            "expected a string or integer error code",
        )),
    }
}
