//! Tests for the CoinMarketCap plugin: settings, command registration, cache construction, name
//! resolution, currency parsing, reply mapping and the clap options.

use std::sync::Arc;
use std::time::Duration;

use coinmarketcap::model::{Coin, DEFAULT_CURRENCY, Fiat};
use coinmarketcap::{Client as CmcClient, ClientOptions, Error};

use zeta_cache::TtlCache;
use zeta_plugin::irc::plain_reply;
use zeta_plugin::{ArgsError, Registered, Subscriptions};
use zeta_test_support::wiremock::{Mock, MockServer, ResponseTemplate, matchers::any};
use zeta_test_support::{refused_get, settings_tests};

use crate::args::{CoinOpts, QuoteOpts};
use crate::cache::{CoinCache, FiatCache};
use crate::commands::{BTC, CC, COIN_COMMANDS, MAX_CURRENCY_CHARS, parse_currency, reply_message};
use crate::{CoinMarketCap, Settings};

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
        client: Arc::new(
            CmcClient::try_new_with_options("test-api-key", &ClientOptions::default()).unwrap(),
        ),
        coins: TtlCache::new_with_value(CoinCache::from(test_coins()), CACHE_TTL),
        fiat: TtlCache::new(CACHE_TTL),
        default_currency: DEFAULT_CURRENCY.to_string(),
        max_name_typos: 2,
    }
}

#[tokio::test]
async fn publishes_its_client_for_other_plugins() {
    // The keyed CoinMarketCap client must be discoverable through the shared state, e.g.
    // by the rink plugin's live cryptocurrency units.
    let ctx = zeta_test_support::test_context();
    let mut subscriptions = Subscriptions::new();
    let settings = Settings {
        api_key: Some("test-api-key".to_string()),
        ..Settings::default()
    };

    <CoinMarketCap as Registered>::new(&ctx, &settings, &mut subscriptions).unwrap();

    assert!(ctx.shared().get::<CmcClient>().is_some());
}

/// The default cache lifetime, as configured in [`Settings::default`].
const CACHE_TTL: Duration = Duration::from_hours(24);

settings_tests! {
    Settings,
    settings,
    default: {
        assert!(settings.api_key.is_none());
        assert_eq!(settings.default_currency, "USD");
        assert_eq!(settings.cache_ttl, Duration::from_hours(24));
        assert_eq!(settings.max_name_typos, 2);
    }
    deserialize: {
        "api_key": "secret",
        "default_currency": "eur",
        "cache_ttl": "1h",
        "max_name_typos": 3,
    } assert: {
        assert_eq!(settings.api_key.as_deref(), Some("secret"));
        assert_eq!(settings.default_currency, "eur");
        assert_eq!(settings.cache_ttl, Duration::from_hours(1));
        assert_eq!(settings.max_name_typos, 3);
    }
}

#[test]
fn commands_are_consistent() {
    let mut triggers = std::iter::once(CC)
        .chain(COIN_COMMANDS.iter().map(|(command, _)| *command))
        .map(|command| command.trigger())
        .collect::<Vec<_>>();
    let count = triggers.len();

    triggers.sort_unstable();
    triggers.dedup();

    assert_eq!(count, triggers.len(), "duplicate command in the table");
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

    assert_eq!(cache.by_symbol["ONE"], 336);
}

#[test]
fn resolves_coins_by_symbol_and_name() {
    let plugin = test_plugin();

    assert_eq!(plugin.resolve_coin("btc"), Some(1));
    assert_eq!(plugin.resolve_coin("BTC"), Some(1));

    // `bitcoin` matches `Bitcoin` rather than the longer `Bitcoin Cash`.
    assert_eq!(plugin.resolve_coin("bitcoin"), Some(1));

    // Misspelled names still resolve, including with substituted characters.
    assert_eq!(plugin.resolve_coin("bitcon"), Some(1));
    assert_eq!(plugin.resolve_coin("bitkoin"), Some(1));
    assert_eq!(plugin.resolve_coin("doge"), Some(74));

    assert_eq!(plugin.resolve_coin(""), None);
    assert_eq!(plugin.resolve_coin("   "), None);
    assert_eq!(plugin.resolve_coin("zzzz"), None);
}

#[test]
fn parses_currencies() {
    let is_valid = |currency: &str| matches!(currency, "USD" | "EUR" | "JPY");

    assert_eq!(parse_currency(None, "USD", is_valid).unwrap(), "USD");
    assert_eq!(parse_currency(Some("eur"), "USD", is_valid).unwrap(), "EUR");
    assert_eq!(parse_currency(Some("JPY"), "USD", is_valid).unwrap(), "JPY");
    assert!(matches!(
        parse_currency(Some("xyz"), "USD", is_valid),
        Err(Error::InvalidCurrency(currency)) if currency == "XYZ"
    ));

    // Without a fiat cache, validation is deferred to the API.
    assert_eq!(parse_currency(Some("xyz"), "USD", |_| true).unwrap(), "XYZ");
}

#[test]
fn validates_currencies_against_the_fiat_cache() {
    let mut plugin = test_plugin();

    // An empty fiat cache defers validation to the API.
    assert!(plugin.is_valid_currency("XYZ"));

    plugin.fiat = TtlCache::new_with_value(
        FiatCache::from(vec![Fiat {
            sign: "$".to_string(),
            symbol: "USD".to_string(),
        }]),
        CACHE_TTL,
    );

    assert!(plugin.is_valid_currency("USD"));
    assert!(!plugin.is_valid_currency("XYZ"));
}

#[test]
fn sanitizes_currencies() {
    // Control characters (which include all IRC formatting bytes) must not survive into
    // the currency that is echoed back on error.
    let is_valid = |currency: &str| currency == "USD";

    assert!(matches!(
        parse_currency(Some("\u{2}xyz\u{f}"), "USD", is_valid),
        Err(Error::InvalidCurrency(currency)) if currency == "XYZ"
    ));

    // A color code's numeric parameters must not leak into the visible currency either.
    assert_eq!(
        parse_currency(Some("\u{3}10usd\u{f}"), "EUR", is_valid).unwrap(),
        "USD"
    );
    assert_eq!(
        parse_currency(Some("\u{4}FF0000usd"), "EUR", is_valid).unwrap(),
        "USD"
    );
}

#[test]
fn maps_lookup_failures_to_reply_grade_messages() {
    // A newline is not a valid header value, so the client rejects the key.
    let invalid_api_key =
        CmcClient::try_new("bad\nkey").expect_err("the api key is not a header value");

    let cases = [
        (Error::NotFound, "no such coin"),
        (
            Error::InvalidCurrency("xyz".to_string()),
            "unsupported currency: xyz",
        ),
        (
            Error::Api("secret body".to_string()),
            "could not retrieve the coin information",
        ),
        (invalid_api_key, "the CoinMarketCap API key is invalid"),
    ];

    for (err, expected) in cases {
        assert_eq!(reply_message(&err), expected, "{err:?}");
    }
}

#[tokio::test]
async fn maps_transport_failures_to_reply_grade_messages() {
    let (error, _) = refused_get("secret").await;
    let build = Error::Build(error.into());
    assert_eq!(
        reply_message(&build),
        "could not reach the CoinMarketCap API"
    );

    let (error, _) = refused_get("secret").await;
    let request = Error::Request(error.into());
    assert_eq!(
        reply_message(&request),
        "could not reach the CoinMarketCap API"
    );
}

#[tokio::test]
async fn maps_deserialize_failures_to_a_reply_grade_message() {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("{"))
        .mount(&server)
        .await;

    let client =
        CmcClient::try_new_with_base("test-api-key", &ClientOptions::default(), &server.uri())
            .unwrap();
    let err = client.fiat_map().await.expect_err("malformed json");

    assert_eq!(
        reply_message(&err),
        "could not parse the response from CoinMarketCap"
    );
}

#[test]
fn keeps_api_text_out_of_the_reply() {
    let err = Error::Api("secret body".to_string());
    let reply = plain_reply(reply_message(&err));

    assert_eq!(
        reply,
        plain_reply("could not retrieve the coin information")
    );
    assert!(!reply.contains("secret body"), "{reply:?}");
}

#[test]
fn sanitizes_the_currency_in_unsupported_currency_replies() {
    // The color code and its parameters must not survive into the reply.
    let err = Error::InvalidCurrency("\u{3}10xyz\u{f}".to_string());
    let reply = reply_message(&err);

    assert_eq!(reply, "unsupported currency: xyz");
    assert!(!reply.chars().any(char::is_control), "{reply:?}");
    assert!(!reply.contains('\u{3}'), "{reply:?}");
}

#[test]
fn bounds_the_currency_in_unsupported_currency_replies() {
    let err = Error::InvalidCurrency("A".repeat(100));
    let reply = reply_message(&err);

    assert!(reply.ends_with('…'), "{reply:?}");
    assert_eq!(
        reply.chars().count(),
        "unsupported currency: ".chars().count() + MAX_CURRENCY_CHARS
    );
}

#[test]
fn parses_command_options() {
    let opts: QuoteOpts = BTC.parse_shell("").unwrap();
    assert_eq!(opts.currency, None);

    let opts: QuoteOpts = BTC.parse_shell("eur").unwrap();
    assert_eq!(opts.currency.as_deref(), Some("eur"));

    let opts: CoinOpts = CC.parse_shell("bitcoin eur").unwrap();
    assert_eq!(opts.coin, "bitcoin");
    assert_eq!(opts.currency.as_deref(), Some("eur"));

    // `.cc` requires a coin.
    assert!(matches!(
        CC.parse_shell::<CoinOpts>(""),
        Err(ArgsError::Usage(_))
    ));
}
