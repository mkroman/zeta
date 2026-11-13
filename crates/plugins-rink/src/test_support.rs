//! Test-only helpers shared by the crate's test modules.
//!
//! Unit-test helpers have to live under `src` to reach the crate's private items. The module is
//! declared under `#[cfg(test)]` in `lib.rs`, and the file-level gate below keeps it out of
//! production builds on its own.

#![cfg(test)]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use plugins_shared::crypto::{CryptoListing, CryptoSource};
use zeta_cache::TtlCache;
use zeta_http::HttpConfig;
use zeta_plugin::{ZetaError, plugin_err};
use zeta_test_support::wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

use crate::Rink;
use crate::actor::Rewrite;
use crate::currency::{self, build_context};

/// Evaluates `line` through the actor, panicking if it has stopped.
pub async fn evaluate(rink: &Rink, line: &str, rewrite: Rewrite) -> String {
    rink.eval_and_format(line, rewrite)
        .await
        .expect("the rink actor must be running")
}

/// A minimal live currency dataset: USD, DKK and JPY against the bundled euro, plus a
/// Bitcoin substance so the subunit expressions resolve.
pub const TEST_CURRENCY_DATA: &str = r#"[
    {
        "name": "USD",
        "doc": null,
        "category": "currencies",
        "type": "unit",
        "expr": "2 EUR"
    },
    {
        "name": "DKK",
        "doc": null,
        "category": "currencies",
        "type": "unit",
        "expr": "(1 / 7.4752) EUR"
    },
    {
        "name": "JPY",
        "doc": null,
        "category": "currencies",
        "type": "unit",
        "expr": "(1 / 170.52) EUR"
    },
    {
        "name": "BTC",
        "doc": null,
        "category": "currencies",
        "type": "unit",
        "expr": "price of bitcoin"
    },
    {
        "name": "bitcoin",
        "doc": "Properties of the global Bitcoin network.",
        "category": "currencies",
        "type": "substance",
        "symbol": null,
        "properties": [
            {
                "name": "price",
                "doc": "Current market price of 1 BTC.",
                "category": "currencies",
                "inputName": "bitcoin",
                "input": "1",
                "outputName": "bitcoin",
                "output": "65000 USD"
            }
        ]
    }
]"#;

/// A ranked cryptocurrency listing fixture: Ethereum and a faked euro coin, plus a symbol
/// rink's loader would reject.
pub fn test_listings() -> Vec<CryptoListing> {
    vec![
        CryptoListing {
            symbol: "ETH".to_string(),
            name: "Ethereum".to_string(),
            usd_price: Some(2650.32),
        },
        CryptoListing {
            symbol: "1INCH".to_string(),
            name: "1inch".to_string(),
            usd_price: Some(0.31),
        },
        CryptoListing {
            symbol: "EUR".to_string(),
            name: "Fake Euro".to_string(),
            usd_price: Some(1.2),
        },
    ]
}

/// A [`CryptoSource`] fake that serves [`test_listings`] or fails, counting its calls.
#[derive(Debug)]
pub struct FakeCryptoSource {
    /// The listings served when the source does not fail.
    listings: Vec<CryptoListing>,
    /// Whether every fetch fails.
    fail: bool,
    /// The number of fetches requested.
    calls: AtomicUsize,
}

impl FakeCryptoSource {
    /// Creates a source serving the cryptocurrency listing fixture.
    #[must_use]
    pub fn serving() -> Arc<Self> {
        Arc::new(Self {
            listings: test_listings(),
            fail: false,
            calls: AtomicUsize::new(0),
        })
    }

    /// Creates a source whose fetches always fail.
    #[must_use]
    pub fn failing() -> Arc<Self> {
        Arc::new(Self {
            listings: Vec::new(),
            fail: true,
            calls: AtomicUsize::new(0),
        })
    }

    /// Returns how many times the source was asked for listings.
    #[must_use]
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

impl CryptoSource for FakeCryptoSource {
    fn listings<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<CryptoListing>, ZetaError>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::Relaxed);

        let result = if self.fail {
            Err(plugin_err(std::io::Error::other(
                "the fake crypto source failed",
            )))
        } else {
            Ok(self.listings.clone())
        };

        Box::pin(std::future::ready(result))
    }
}

/// Builds a plugin with a bundled-only context, with currency fetching enabled and pointed
/// at `url` when given.
pub fn test_plugin(url: Option<String>) -> Rink {
    let mut rink = Rink {
        engine: Some(build_context(None, None).unwrap()),
        actor: None,
        currency: Arc::new(TtlCache::new(Duration::from_mins(10))),
        refresh_task: None,
        client: Some(zeta_http::build_client(&HttpConfig::default())),
        crypto: None,
        currency_url: url.unwrap_or_else(|| currency::CURRENCY_URL.to_string()),
        crypto_enabled: true,
    };

    rink.spawn_actor();

    rink
}

/// Builds a plugin with currency fetching disabled.
pub fn offline_plugin() -> Rink {
    let mut rink = Rink {
        engine: Some(build_context(None, None).unwrap()),
        actor: None,
        currency: Arc::new(TtlCache::new(Duration::from_mins(10))),
        refresh_task: None,
        client: None,
        crypto: None,
        currency_url: currency::CURRENCY_URL.to_string(),
        crypto_enabled: true,
    };

    rink.spawn_actor();

    rink
}

/// Starts a wiremock server serving `TEST_CURRENCY_DATA` at the currency path.
pub async fn currency_server(status: u16) -> MockServer {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/data/currency.json"))
        .respond_with(ResponseTemplate::new(status).set_body_string(TEST_CURRENCY_DATA))
        .mount(&server)
        .await;

    server
}

/// Builds a plugin with the cryptocurrency source serving the listing fixture (`serving`) or
/// failing, and the fiat dataset served by the returned fiat server.
///
/// The server travels with the plugin because dropping a `MockServer` returns it to wiremock's
/// pool, where another test can acquire it: the pooled server is reset, unmounting these mocks,
/// and this plugin's requests would then be recorded against whichever test happened to borrow
/// it.
pub async fn plugin_with_listings(serving: bool) -> (Rink, MockServer, Arc<FakeCryptoSource>) {
    let fiat_server = currency_server(200).await;
    let source = if serving {
        FakeCryptoSource::serving()
    } else {
        FakeCryptoSource::failing()
    };

    let mut rink = test_plugin(Some(format!("{}/data/currency.json", fiat_server.uri())));
    rink.crypto = Some(source.clone());

    (rink, fiat_server, source)
}
