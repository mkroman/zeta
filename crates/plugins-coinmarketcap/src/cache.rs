//! The cached coins and fiat currencies, and the lookups that read them.
//!
//! Both caches are populated from their map endpoints and refreshed lazily after the cache TTL.
//! A failed refresh leaves the existing cache, if any, in place: coin names keep resolving with
//! the stale coins, and an empty fiat cache defers currency validation to the API.

use std::collections::HashMap;

use tracing::{debug, warn};

use zeta_plugin::Chain;

use coinmarketcap::Error;
use coinmarketcap::model::{Coin, Fiat};

use crate::CoinMarketCap;
use crate::matching::{self, NamedCoin};

/// The top cryptocurrencies by market cap, keyed by ticker symbol.
///
/// Populated from the cryptocurrency map endpoint at startup and refreshed lazily after the
/// cache TTL expires.
#[derive(Debug)]
pub(super) struct CoinCache {
    /// The cached coin ids, keyed by ticker symbol.
    ///
    /// Only the id is read back, so the symbol is the single copy the cache keeps: it is moved
    /// into the map key instead of being cloned next to a stored `Coin`.
    pub(super) by_symbol: HashMap<String, u64>,
    /// The coins' names, lower-cased once for fuzzy matching.
    names: Vec<NamedCoin>,
}

impl From<Vec<Coin>> for CoinCache {
    fn from(coins: Vec<Coin>) -> Self {
        // The map endpoint returns coins in ascending rank order; keep the first (highest
        // ranked) occurrence when multiple coins share a symbol.
        let mut by_symbol: HashMap<String, u64> = HashMap::with_capacity(coins.len());
        let mut names = Vec::with_capacity(coins.len());

        for coin in coins {
            if !by_symbol.contains_key(coin.symbol.as_str()) {
                names.push(NamedCoin::new(&coin));
                by_symbol.insert(coin.symbol, coin.id);
            }
        }

        Self { by_symbol, names }
    }
}

/// The fiat currencies supported by the API for price conversion.
///
/// Populated from the fiat map endpoint at startup and refreshed lazily after the cache TTL
/// expires.
#[derive(Debug)]
pub(super) struct FiatCache {
    /// The currency sign of each symbol (e.g. `USD` maps to `$`).
    signs: HashMap<String, String>,
}

impl From<Vec<Fiat>> for FiatCache {
    fn from(fiats: Vec<Fiat>) -> Self {
        Self {
            signs: fiats
                .into_iter()
                .map(|fiat| (fiat.symbol, fiat.sign))
                .collect(),
        }
    }
}

impl CoinMarketCap {
    /// Resolves a user query to a coin id, by exact symbol match first and by fuzzy name match
    /// second.
    pub(super) fn resolve_coin(&self, query: &str) -> Option<u64> {
        self.coins.with_value(|cache| {
            let cache = cache?;

            cache
                .by_symbol
                .get(&query.to_ascii_uppercase())
                .copied()
                .or_else(|| matching::find_by_name(&cache.names, query, self.max_name_typos))
        })
    }

    /// Refreshes the coin cache when it is missing or older than the cache TTL.
    ///
    /// Failures are logged and leave the existing cache, if any, in place: names keep
    /// resolving with the stale coins, and an empty cache falls back to letting the API
    /// resolve queries as symbols.
    pub(super) async fn ensure_coins_cached(&self) {
        if let Err(err) = self
            .coins
            .refresh(|| async {
                let coins = self.client.coin_map().await?;
                let count = coins.len();
                debug!(
                    coinmarketcap.count = count,
                    "cached the top cryptocurrencies"
                );

                Ok::<_, Error>(CoinCache::from(coins))
            })
            .await
        {
            warn!(
                exception.message = %Chain(&err),
                "could not cache the top cryptocurrencies"
            );
        }
    }

    /// Refreshes the fiat currency cache when it is missing or older than the cache TTL.
    ///
    /// Failures are logged and leave the existing cache, if any, in place: quotes keep working
    /// with the stale currencies, and an empty cache defers currency validation to the API.
    pub(super) async fn ensure_fiat_cached(&self) {
        if let Err(err) = self
            .fiat
            .refresh(|| async {
                let fiats = self.client.fiat_map().await?;
                let count = fiats.len();
                debug!(
                    coinmarketcap.count = count,
                    "cached the supported fiat currencies"
                );

                Ok::<_, Error>(FiatCache::from(fiats))
            })
            .await
        {
            warn!(
                exception.message = %Chain(&err),
                "could not cache the supported fiat currencies"
            );
        }
    }

    /// Returns whether `currency` is a fiat currency supported for price conversion.
    ///
    /// Returns `true` while the fiat cache has not been populated, deferring validation to
    /// the API.
    pub(super) fn is_valid_currency(&self, currency: &str) -> bool {
        self.fiat.with_value(|cache| {
            cache.is_none_or(|cache| cache.signs.is_empty() || cache.signs.contains_key(currency))
        })
    }

    /// Returns the currency sign for `currency`, if known.
    pub(super) fn fiat_sign(&self, currency: &str) -> Option<String> {
        self.fiat
            .with_value(|cache| cache.and_then(|cache| cache.signs.get(currency).cloned()))
    }
}
