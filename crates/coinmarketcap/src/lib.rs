//! A client for the CoinMarketCap Pro API.
//!
//! This crate provides an asynchronous [`Client`] for the [CoinMarketCap](https://coinmarketcap.com)
//! Pro API: the cryptocurrency map and ranked listings, the fiat map, and per-coin quotes, with
//! the API key carried in a request header.
//!
//! ## Features
//!
//! - **`default-tls`** *(enabled by default)*: selects the default TLS backend (**`rustls`**).
//!   Disable default features and choose a TLS backend explicitly with **`rustls`**,
//!   **`native-tls`** or **`native-tls-vendored`**.
//! - **`gzip`**, **`brotli`**, **`deflate`**, **`zstd`** *(all enabled by default)*:
//!   decompression of compressed responses.
//! - **`log`**: logs each failed request with its query-free URL, so a logged error never
//!   carries a credential.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use coinmarketcap::{Client, Config};
//!
//! # async fn example() -> Result<(), coinmarketcap::Error> {
//! let client = Client::new(&Config {
//!     api_key: "coinmarketcap api key".to_string(),
//!     timeout: std::time::Duration::from_secs(30),
//!     user_agent: "zeta/0.12".to_string(),
//! })?;
//!
//! let listings = client.listings(100).await?;
//!
//! if let Some(bitcoin) = listings.iter().find(|listing| listing.symbol == "BTC") {
//!     println!("Bitcoin ranks {}", bitcoin.cmc_rank.unwrap_or(0));
//! }
//! # Ok(())
//! # }
//! ```

// Allow repetition of structure name instead of replacing with self as the output from
// rust-analyzer becomes more readable
#![allow(clippy::use_self)]

mod client;
mod error;
pub mod model;

pub use client::{Client, Config};
pub use error::Error;
pub use model::{
    Coin, CoinQuery, DEFAULT_CURRENCY, Envelope, Fiat, FiatQuote, Listing, ListingQuote, QuoteData,
    Status,
};
