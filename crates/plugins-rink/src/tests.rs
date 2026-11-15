//! Calculation, currency, and settings tests for the rink plugin.

use super::*;
use crate::actor::Rewrite;
use zeta_plugin::{Registered, Subscriptions};
use zeta_test_support::settings_tests;
use zeta_utils::irc;

use crate::test_support::{
    TEST_CURRENCY_DATA, currency_server, evaluate, listings_server, offline_plugin,
    plugin_with_listings, test_client_options, test_plugin,
};

#[tokio::test]
async fn ans_references_the_previous_result() {
    let rink = offline_plugin();

    evaluate(&rink, "4 m", Rewrite::Verbatim).await;

    let reply = irc::strip_formatting(&evaluate(&rink, "ans * 2", Rewrite::Verbatim).await);
    assert!(reply.contains("8 meter"), "{reply}");
    assert!(reply.contains("length"), "{reply}");
}

#[tokio::test]
async fn the_extra_units_load_and_resolve() {
    let rink = offline_plugin();

    let cases = [
        ("300 px -> mm", &["79.375 millimeter", "length"][..]),
        ("90 grad -> degree", &["81 degree", "angle"][..]),
        (
            "300 dpi -> dpcm",
            &["15000/127", "118.1102 dpcm", "m^-1"][..],
        ),
        ("1 dppx -> dpi", &["96 dpi", "m^-1"][..]),
    ];

    for (line, expected) in cases {
        let reply = irc::strip_formatting(&evaluate(&rink, line, Rewrite::Verbatim).await);

        for needle in expected {
            assert!(
                reply.contains(needle),
                "{line}: missing {needle:?}: {reply}"
            );
        }
    }
}

#[tokio::test]
async fn crypto_units_and_subunits_convert() {
    let (rink, _fiat, _listings) = plugin_with_listings(200).await;

    rink.ensure_currency_cached().await;

    for (line, rewrite) in [
        ("1 ETH to USD", Rewrite::Verbatim),
        ("1 eth to usd", Rewrite::Classify),
    ] {
        let reply = irc::strip_formatting(&evaluate(&rink, line, rewrite).await);
        assert!(reply.contains("2650.32 USD"), "{line}: {reply}");
        assert!(reply.contains("money"), "{line}: {reply}");
    }

    // 50_000 satoshis are 0.0005 bitcoin at the fixture's 65 000 USD price.
    let sats =
        irc::strip_formatting(&evaluate(&rink, "50000 sats to usd", Rewrite::Classify).await);
    assert!(sats.contains("32.5 USD"), "{sats}");

    // A gwei stays convertible without being resolvable as fiat.
    let gwei = irc::strip_formatting(&evaluate(&rink, "1 gwei to usd", Rewrite::Classify).await);
    assert!(gwei.contains("USD"), "{gwei}");
}

#[tokio::test]
async fn synthesis_skips_colliding_and_invalid_symbols() {
    let (rink, _fiat, _listings) = plugin_with_listings(200).await;

    rink.ensure_currency_cached().await;

    // The fake EUR coin (priced 1.2) must not shadow the bundled euro: 1 EUR is 0.5 USD
    // under the fiat fixture, and the rejected `1INCH` symbol must not break the load.
    let reply = irc::strip_formatting(&evaluate(&rink, "1 EUR to USD", Rewrite::Verbatim).await);
    assert!(reply.contains("0.5 USD"), "{reply}");
    assert!(!reply.contains("1.2"), "{reply}");
}

#[test]
fn a_duplicate_ticker_keeps_the_whole_context_build_loadable() {
    use coinmarketcap::model::{Envelope, Listing};

    // Two listings sharing a ticker: rink rejects a definitions batch that defines the
    // same name twice, and a rejected crypto batch used to fail the whole build — taking
    // the fiat data fetched alongside it down too.
    let text = r#"{
        "data": [
            {"id":1,"name":"Foo","symbol":"FOO","slug":"foo","cmc_rank":1,
             "quote":[{"id":2781,"symbol":"USD","price":1.0}]},
            {"id":2,"name":"Bar","symbol":"FOO","slug":"bar","cmc_rank":2,
             "quote":[{"id":2781,"symbol":"USD","price":2.0}]}
        ],
        "status":{"timestamp":"2026-09-29T00:00:00Z","error_code":"0",
                  "error_message":null,"elapsed":1,"credit_count":1}
    }"#;
    let parsed: Envelope<Vec<Listing>> =
        zeta_http::json::from_str(text).expect("the fixture should decode");
    let data = CurrencyData {
        fiat: TEST_CURRENCY_DATA.to_string(),
        crypto: parsed.data,
    };

    let ctx = build_context(Some(&data), None)
        .expect("a duplicate ticker must not fail the whole context build");

    // The fiat dataset landed, and the higher-ranked coin of the pair survived.
    assert!(ctx.lookup("USD").is_some(), "the fiat data must still load");
    assert!(
        ctx.lookup("FOO").is_some(),
        "the first-ranked coin must load"
    );
}

#[tokio::test]
async fn missing_crypto_answers_missing_dependencies() {
    // No coinmarketcap client was published: the crypto units stay unloaded even though
    // the crypto setting is on. The subunits' pending dependency declaration surfaces ETH
    // in the missing-dependencies error.
    let mut rink = offline_plugin();
    rink.crypto_enabled = true;
    rink.cmc = None;
    rink.ensure_currency_cached().await;

    let disabled = irc::strip_formatting(&evaluate(&rink, "1 eth to usd", Rewrite::Classify).await);
    assert!(disabled.contains("Missing dependencies: ETH"), "{disabled}");

    // The fiat dataset is fine; the listings endpoint is not. The refresh fails as a
    // whole, so the last known context — without crypto — stays in place, and the
    // pending dependency still surfaces ETH.
    let (rink, _fiat, _listings) = plugin_with_listings(500).await;
    rink.ensure_currency_cached().await;

    let failed = irc::strip_formatting(&evaluate(&rink, "1 eth to usd", Rewrite::Classify).await);
    assert!(failed.contains("Missing dependencies: ETH"), "{failed}");
}

#[tokio::test]
async fn a_pending_crypto_table_is_retried_inline() {
    let fiat = currency_server(200).await;
    let listings = listings_server(500).await;

    let mut rink = test_plugin(Some(format!("{}/data/currency.json", fiat.uri())));
    rink.cmc = Some(Arc::new(
        CmcClient::try_new_with_base("test-api-key", &test_client_options(), &listings.uri())
            .unwrap(),
    ));

    rink.ensure_currency_cached().await;
    assert_eq!(listings.received_requests().await.unwrap().len(), 1);

    // The listings fetch failed, so the requested table is absent even though the fiat
    // entry is fresh: unlike an unresolvable dependency, this state a refetch can fix,
    // so the query retries the listings inline instead of waiting out the whole TTL.
    let reply = evaluate(&rink, "1 gwei to usd", Rewrite::Classify).await;
    assert!(reply.contains("Missing dependencies"), "{reply}");

    assert_eq!(listings.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn missing_fiat_answers_missing_dependencies() {
    let server = currency_server(500).await;
    let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));

    let failed = irc::strip_formatting(&evaluate(&rink, "1 USD", Rewrite::Verbatim).await);
    assert!(failed.contains("Missing dependencies: USD"), "{failed}");

    let rink = offline_plugin();

    let disabled = irc::strip_formatting(&evaluate(&rink, "1 USD", Rewrite::Verbatim).await);
    assert!(disabled.contains("Missing dependencies: USD"), "{disabled}");
}

#[tokio::test]
async fn a_fresh_cache_does_not_refetch_for_an_unresolvable_dependency() {
    let server = currency_server(200).await;
    let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));

    // The cold cache fetches once, as it always has.
    rink.ensure_currency_cached().await;
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    // `gwei` rests on the ETH dependency, which no coinmarketcap client can resolve
    // here, so it stays pending — but the cached dataset is fresh and re-fetching it
    // cannot add the missing table. The queries must not reach the network again.
    for _ in 0..3 {
        // `.c` case-corrects `usd`, so the pending `gwei` dependency is what surfaces.
        let reply = evaluate(&rink, "1 gwei to usd", Rewrite::Classify).await;

        assert!(reply.contains("Missing dependencies"), "{reply}");
    }

    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "needs network access"]
async fn the_live_dataset_serves_fiat_and_bitcoin_queries() {
    let rink = test_plugin(None);

    rink.ensure_currency_cached().await;

    let fiat = evaluate(&rink, "100 usd to eur", Rewrite::Classify).await;
    assert!(fiat.contains("euro") && fiat.contains("money"), "{fiat}");

    let bitcoin = evaluate(&rink, "1 btc to usd", Rewrite::Classify).await;
    assert!(
        bitcoin.contains("USD") && bitcoin.contains("money"),
        "{bitcoin}"
    );
}

#[tokio::test]
async fn the_plugin_registers_its_commands() {
    let mut subscriptions = Subscriptions::new();

    <Rink as Registered>::new(
        &zeta_test_support::test_context(),
        &Settings::default(),
        &mut subscriptions,
    )
    .unwrap();

    assert!(subscriptions.commands().contains(&RINK));
    assert!(subscriptions.commands().contains(&CLASSIFY));
}

settings_tests! {
    Settings,
    settings,
    default: {
        assert!(settings.currency);
        assert_eq!(settings.currency_ttl, Duration::from_hours(1));
        assert_eq!(settings.currency_url, currency::CURRENCY_URL);
    }
    deserialize: {
        "currency": false,
        "currency_ttl": "30min",
        "currency_url": "https://example.invalid/currency.json",
    } assert: {
        assert!(!settings.currency);
        assert_eq!(settings.currency_ttl, Duration::from_mins(30));
        assert_eq!(settings.currency_url, "https://example.invalid/currency.json");
    }
}
