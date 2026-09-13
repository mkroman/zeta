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

/// A quote in a single conversion currency.
#[derive(Debug, Deserialize)]
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
    /// The error code; `0` indicates success.
    pub error_code: i32,
    /// A human-readable description of the error, if any.
    pub error_message: Option<String>,
}
