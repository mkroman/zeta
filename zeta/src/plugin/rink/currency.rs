//! The live currency dataset powering the rink plugin's money units.
//!
//! Two sources feed the context: rink's own dataset (fiat reference rates from the European
//! Central Bank, plus Bitcoin properties from blockchain.info) and the CoinMarketCap top
//! listings (the cryptocurrency table). The rink dataset is required; the listings refresh
//! warns and keeps the previous table when it fails.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use std::fmt::Write;

use rink_core::types::DateTime;
use rink_core::{Context as RinkContext, Value};
use tracing::{debug, warn};

use crate::plugin::coinmarketcap::{client, model::Listing};
use crate::plugin::prelude::*;
use crate::utils::strip_control_chars;

/// The URL of rink's live currency dataset.
///
/// It updates about once an hour: fiat rates sourced from the European Central Bank, and
/// Bitcoin network and price properties sourced from blockchain.info.
pub const CURRENCY_URL: &str = "https://rinkcalc.app/data/currency.json";

/// The time-to-live of the fetched currency data, matching the dataset's update cadence.
pub const CURRENCY_TTL: Duration = Duration::from_hours(1);

/// How often the currency task rechecks the cache for staleness.
///
/// A tick only fetches when the cached entry is missing or past its TTL, so the actual fetches
/// happen at most once per [`CURRENCY_TTL`]; the shorter check interval makes a failed fetch
/// retry within minutes instead of after a full TTL.
pub const CURRENCY_CHECK_INTERVAL: Duration = Duration::from_mins(10);

/// The number of top cryptocurrencies (by market cap) fetched for the money unit table.
///
/// The CoinMarketCap endpoint is charged one call credit per 250 coins returned (rounded up),
/// so a limit of 100 costs a single credit per refresh.
const CRYPTO_LISTINGS_LIMIT: u32 = 100;

/// The static cryptocurrency subunit names defined in [`EXTRA_UNITS`]; a coin symbol that
/// would shadow one of them is skipped by the synthesizer.
const CRYPTO_SUBUNIT_NAMES: &[&str] = &[
    "satoshi", "sats", "wei", "gwei", "lamport", "lovelace", "stroop", "piconero", "koinu",
    "litoshi",
];

/// The dataset the current rink context was built from: rink's fiat dataset and, when the
/// coinmarketcap plugin is configured, the cryptocurrency listing table.
#[derive(Clone)]
pub struct CurrencyData {
    /// The body of rink's live currency dataset.
    pub fiat: String,
    /// The ranked cryptocurrency listings, when the coinmarketcap plugin is available.
    pub crypto: Option<Vec<Listing>>,
}

/// Extra unit definitions loaded on top of rink's bundled dataset.
///
/// The CSS lengths, resolutions, and angles. The CSS pixel is a length, while the densities are
/// built on the dimensionless `dot` — the "dot" of CSS's `<resolution>` units — so that density
/// conversions (`dpi` to `dpcm`) and screen sizing (`1920 dot / (300 dpi) -> inch`) both come
/// out right. The CSS names that collide with rink's existing names (`Q`, the quetta prefix
/// symbol; `pt`, the pint; `pc`, the parsec; and the font-relative `em` family, which cannot be
/// a static unit at all) are spelled out or left out.
///
/// The cryptocurrency subunits reference the live coin units, so they stay pending until the
/// listing containing each coin is loaded.
pub const EXTRA_UNITS: &str = r#"
!category css                                "CSS Units"

?? The CSS reference pixel, defined by CSS Values and Units Level 3 as
?? 1/96 of a CSS inch, and related to a visual angle of about 0.0213
?? degrees at nominal viewing distance.
pixel                    1|96 inch
px                       pixel

?? The device or print pixel: a dimensionless count, the "dot" of CSS's
?? <resolution> units. Kept distinct from the CSS pixel (a length) so
?? density conversions stay conformal.
dot                      1

?? CSS <resolution> units. CSS defines 1dppx = 96dpi exactly.
dpi                      dot / inch
dpcm                     dot / centimeter
dppx                     dot / pixel

?? The CSS point. The unit exists as `point`, but the CSS name `pt` is
?? taken by the pint and a unit cannot join two categories, so the
?? alias is left to rink's own naming.
quartermm                0.25 mm

?? The CSS angle unit. Rink spells it `grade`/`gon`; naming `grad` also
?? stops it from parsing as gram*radian.
grad                     grade

!endcategory

!category crypto_units                      "Cryptocurrency Units"

!dependency ETH
!dependency SOL
!dependency ADA
!dependency XLM
!dependency XMR
!dependency DOGE
!dependency LTC

?? A satoshi is the smallest denomination of bitcoin.
satoshi                  1e-8 BTC
sats                     satoshi

?? A wei is the smallest Ethereum denomination.
wei                      1e-18 ETH
?? A gwei is a common Ethereum gas price denomination.
gwei                     1e-9 ETH

?? A lamport is the smallest Solana denomination.
lamport                  1e-9 SOL
?? A lovelace is the smallest Cardano denomination.
lovelace                 1e-6 ADA
?? A stroop is the smallest Stellar lumen denomination.
stroop                   1e-7 XLM
?? A piconero is the smallest Monero denomination.
piconero                 1e-12 XMR
?? A koinu is the smallest Dogecoin denomination.
koinu                    1e-8 DOGE
?? A litoshi is the smallest Litecoin denomination.
litoshi                  1e-8 LTC

!endcategory
"#;

/// Fetches the fiat dataset and the cryptocurrency listings, then rebuilds and swaps a fresh
/// context into `ctx`.
///
/// The fiat dataset is required — a failed fetch fails the whole refresh, leaving the previous
/// context in place. A failed listings fetch is logged and the previous cryptocurrency table is
/// kept instead, so a temporary CoinMarketCap outage does not drop the crypto units.
///
/// The previous result (`ans`) is carried into the new context, since a query may reference it
/// across a refresh. Returns the fetched data so the caller's cache tracks the dataset the
/// context was built with.
///
/// # Errors
///
/// Returns a [`ZetaError`] when the fiat fetch fails, the response carries an error status, or
/// the datasets cannot be loaded into a context.
pub async fn refresh_data(
    client: &reqwest::Client,
    currency: &crate::cache::TtlCache<CurrencyData>,
    cmc: Option<&Arc<client::Client>>,
    url: &str,
    crypto_enabled: bool,
    ctx: &Mutex<RinkContext>,
) -> Result<CurrencyData, ZetaError> {
    let fiat = fetch_fiat(client, url).await?;

    let crypto = if crypto_enabled {
        match cmc {
            Some(cmc) => match cmc.listings(CRYPTO_LISTINGS_LIMIT).await {
                Ok(listings) => Some(listings),
                Err(error) => {
                    warn!(
                        %error,
                        "could not refresh cryptocurrency listings; keeping the previous table"
                    );

                    currency.read(|data| data.and_then(|data| data.crypto.clone()))
                }
            },
            None => None,
        }
    } else {
        None
    };

    let previous = crate::sync::lock(ctx).previous_result.clone();
    let next = build_context(
        Some(&CurrencyData {
            fiat: fiat.clone(),
            crypto: crypto.clone(),
        }),
        previous,
    )?;

    *crate::sync::lock(ctx) = next;
    debug!("loaded live currency data into the rink context");

    Ok(CurrencyData { fiat, crypto })
}

/// Fetches the fiat dataset body from `url`.
///
/// # Errors
///
/// Returns a [`ZetaError`] when the fetch fails or the response carries an error status.
async fn fetch_fiat(client: &reqwest::Client, url: &str) -> Result<String, ZetaError> {
    let response = crate::http::send(client.get(url)).await.map_err(plugin_err)?;
    let status = response.status();

    let body = crate::http::text(response).await.map_err(plugin_err)?;

    if !status.is_success() {
        return Err(plugin_err(std::io::Error::other(format!(
            "the currency dataset returned status {status}"
        ))));
    }

    Ok(body)
}

/// Builds a rink context from the bundled dataset, the fetched currency data, and the extra
/// unit definitions.
///
/// Evaluation keeps saving results so `ans` resolves, and `previous` carries the result across
/// context rebuilds. The cryptocurrency listings are synthesized after the fiat load, so coins
/// whose symbol would shadow an already-known unit are skipped.
///
/// # Errors
///
/// Returns a [`ZetaError`] when the bundled dataset, the currency data, or the extra
/// definitions cannot be loaded.
pub fn build_context(
    data: Option<&CurrencyData>,
    previous: Option<Value>,
) -> Result<RinkContext, ZetaError> {
    let mut ctx =
        rink_core::simple_context().map_err(|error| plugin_err(std::io::Error::other(error)))?;
    ctx.save_previous_result = true;
    ctx.previous_result = previous;

    let currency_units = rink_core::CURRENCY_FILE.expect("bundle-files feature to be enabled");
    let fiat = data.map(|data| data.fiat.as_str());
    ctx.load_currency(fiat, currency_units)
        .map_err(|error| plugin_err(std::io::Error::other(error)))?;

    if let Some(listings) = data.and_then(|data| data.crypto.as_ref()) {
        let text = synthesize_crypto_units(&ctx, listings);

        if !text.is_empty() {
            ctx.load_definitions(&text)
                .map_err(|error| plugin_err(std::io::Error::other(error)))?;
        }
    }

    ctx.load_definitions(EXTRA_UNITS)
        .map_err(|error| plugin_err(std::io::Error::other(error)))?;

    Ok(ctx)
}

/// Synthesizes gnu-units definitions from the cryptocurrency listings: one money unit per coin
/// quoted in USD.
///
/// Coins whose symbol would fail rink's loader (e.g. `1INCH`), shadow an already-known unit
/// (the euro, the liquid drop), or carry no USD price are skipped. The returned text is empty
/// when nothing qualifies.
fn synthesize_crypto_units(ctx: &RinkContext, listings: &[Listing]) -> String {
    let date = DateTime::now();
    let mut text = String::new();

    for listing in listings {
        let Some(quote) = listing
            .quote
            .iter()
            .find(|quote| quote.symbol == "USD")
            .and_then(|quote| quote.price)
        else {
            continue;
        };

        if !valid_unit_symbol(&listing.symbol)
            || ctx.lookup(&listing.symbol).is_some()
            || CRYPTO_SUBUNIT_NAMES.contains(&listing.symbol.to_ascii_lowercase().as_str())
        {
            continue;
        }

        let name = strip_control_chars(&listing.name);
        let _ = writeln!(
            text,
            "?? {name} ({}). Sourced from CoinMarketCap. Current as of {date}.",
            listing.symbol
        );
        let _ = writeln!(text, "{} {quote} USD", listing.symbol);
    }

    text
}

/// Returns whether `symbol` can be a unit definition name: at least two ASCII alphanumerics
/// led by a letter (e.g. `ETH`, but not `1INCH`).
fn valid_unit_symbol(symbol: &str) -> bool {
    let mut chars = symbol.chars();

    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && symbol.chars().all(|c| c.is_ascii_alphanumeric())
}
