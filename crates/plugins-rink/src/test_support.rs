//! Test-only helpers shared by the crate's test modules.
//!
//! Unit-test helpers have to live under `src` to reach the crate's private items. The module is
//! declared under `#[cfg(test)]` in `lib.rs`, and the file-level gate below keeps it out of
//! production builds on its own.

#![cfg(test)]

use std::sync::Arc;
use std::time::Duration;

use coinmarketcap::Client as CmcClient;
use zeta_cache::TtlCache;
use zeta_http::HttpConfig;
use zeta_test_support::wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

use crate::Rink;
use crate::currency::{self, build_context};

/// Evaluates `line` through the actor, panicking if it has stopped.
pub async fn evaluate(rink: &Rink, line: &str, classify: bool) -> String {
    rink.eval_and_format(line, classify)
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

/// Builds a CoinMarketCap client configuration for the tests.
pub fn test_client_options() -> coinmarketcap::ClientOptions {
    coinmarketcap::ClientOptions::default()
}

/// A ranked cryptocurrency listing fixture: Ethereum and a faked euro coin, plus a symbol
/// rink's loader would reject.
pub const TEST_LISTINGS_DATA: &str = r#"{
    "data": [
        {
            "id": 1027,
            "name": "Ethereum",
            "symbol": "ETH",
            "slug": "ethereum",
            "cmc_rank": 2,
            "quote": [
                {
                    "id": 2781,
                    "symbol": "USD",
                    "price": 2650.32,
                    "last_updated": "2026-09-29T11:13:00.000Z"
                }
            ]
        },
        {
            "id": 8017,
            "name": "1inch",
            "symbol": "1INCH",
            "slug": "1inch",
            "quote": [
                {
                    "id": 2781,
                    "symbol": "USD",
                    "price": 0.31
                }
            ]
        },
        {
            "id": 1,
            "name": "Fake Euro",
            "symbol": "EUR",
            "slug": "fake-euro",
            "quote": [
                {
                    "id": 2781,
                    "symbol": "USD",
                    "price": 1.2
                }
            ]
        }
    ],
    "status": {
        "timestamp": "2026-09-29T11:13:55.525Z",
        "error_code": "0",
        "error_message": null,
        "elapsed": 12,
        "credit_count": 1
    }
}"#;

/// Builds a plugin with a bundled-only context, with currency fetching enabled and pointed
/// at `url` when given.
pub fn test_plugin(url: Option<String>) -> Rink {
    let mut rink = Rink {
        engine: Some(build_context(None, None).unwrap()),
        actor: None,
        currency: Arc::new(TtlCache::new(Duration::from_mins(10))),
        refresh_task: None,
        client: Some(zeta_http::build_client(&HttpConfig::default())),
        cmc: None,
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
        cmc: None,
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

/// Starts a wiremock server serving `TEST_LISTINGS_DATA` at the CoinMarketCap listings
/// path.
pub async fn listings_server(status: u16) -> MockServer {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v3/cryptocurrency/listings/latest"))
        .respond_with(ResponseTemplate::new(status).set_body_string(TEST_LISTINGS_DATA))
        .mount(&server)
        .await;

    server
}

/// Builds a plugin whose CoinMarketCap client issues requests against the returned
/// listings server, with the fiat dataset served by the returned fiat server.
///
/// The servers travel with the plugin because dropping a `MockServer` returns it to
/// wiremock's pool, where another test can acquire it: the pooled server is reset,
/// unmounting these mocks, and this plugin's requests would then be recorded against
/// whichever test happened to borrow it.
pub async fn plugin_with_listings(status: u16) -> (Rink, MockServer, MockServer) {
    let fiat_server = currency_server(200).await;
    let listings_server = listings_server(status).await;

    let mut rink = test_plugin(Some(format!("{}/data/currency.json", fiat_server.uri())));
    rink.cmc = Some(Arc::new(
        CmcClient::with_base(
            "test-api-key",
            &test_client_options(),
            &listings_server.uri(),
        )
        .unwrap(),
    ));

    (rink, fiat_server, listings_server)
}
