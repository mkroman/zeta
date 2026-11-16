//! The clap arguments of the `.cc` and fixed coin commands.
//!
//! Both command families take an optional fiat currency to convert the price into; `.cc` also
//! takes the coin to quote, by symbol or fuzzy name.

use clap::Parser;

/// Convert a coin price into another fiat currency.
#[derive(Parser, Debug)]
pub(super) struct QuoteOpts {
    /// fiat currency to convert the price into (defaults to USD)
    pub(super) currency: Option<String>,
}

/// Convert a cryptocurrency price into fiat currency.
#[derive(Parser, Debug)]
pub(super) struct CoinOpts {
    /// the coin to quote, by symbol or name
    pub(super) coin: String,
    /// fiat currency to convert the price into (defaults to USD)
    pub(super) currency: Option<String>,
}
