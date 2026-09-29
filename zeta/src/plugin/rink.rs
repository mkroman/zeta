//! Evaluates calculations with unit and currency conversions through rink.
//!
//! The `.r <expression>` command evaluates the whole argument as a rink expression and replies
//! with the one-line result as a notice — arithmetic, physical unit conversions, and, with live
//! currency data loaded, fiat currency and Bitcoin lookups. Evaluation errors are reported
//! inline, with rink's error text rendered in red.
//!
//! Replies are styled like the other plugins: the cyan `> ` marker and message text, with the
//! values — numbers, datetimes, and the user's echoed input in errors — reset to the default
//! color to stand out. Rink's output is rendered from its markup spans rather than its plain
//! text form, so the styling can follow the structure of the answer.
//!
//! A rink context is built once at plugin initialization: the bundled unit definitions, the
//! static currency units, and a small set of extra definitions (CSS lengths, resolutions, and
//! angles) loaded on top. Evaluation calls are serialized through it, and a failed context
//! build aborts plugin initialization.
//!
//! When the `currency` setting is enabled (the default), live currency data is fetched from
//! rink's dataset — fiat rates from the European Central Bank and Bitcoin from blockchain.info
//! — by a background task started when the plugin loads, and rebuilt into the context once per
//! `currency_ttl`. A query that runs into missing currency data triggers one inline fetch
//! attempt and a re-evaluation, mirroring upstream rink's REPL; failed fetches are logged and
//! leave the last known rates in place.

use std::fmt::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rink_core::output::fmt::{FmtToken, Span, TokenFmt};
use rink_core::output::{QueryError, QueryReply};
use rink_core::{Context as RinkContext, Value};
use serde::{Deserialize, Serialize};
use tokio::time::MissedTickBehavior;
use tracing::{Instrument, debug, warn};

use crate::{
    cache::TtlCache,
    error::RequestError,
    http,
    plugin::prelude::*,
    utils::collapse_whitespace,
};

/// The URL of rink's live currency dataset.
///
/// It updates about once an hour: fiat rates sourced from the European Central Bank, and
/// Bitcoin network and price properties sourced from blockchain.info.
const CURRENCY_URL: &str = "https://rinkcalc.app/data/currency.json";

/// The time-to-live of the fetched currency data, matching the dataset's update cadence.
const CURRENCY_TTL: Duration = Duration::from_hours(1);

/// How often the currency task rechecks the cache for staleness.
///
/// A tick only fetches when the cached entry is missing or past its TTL, so the actual fetches
/// happen at most once per [`CURRENCY_TTL`]; the shorter check interval makes a failed fetch
/// retry within minutes instead of after a full TTL.
const CURRENCY_CHECK_INTERVAL: Duration = Duration::from_mins(10);

/// The red (mIRC color 4) that rink's error text renders in, as in the coinmarketcap plugin.
const ERROR_COLOR: &str = "\x034";

/// Extra unit definitions loaded on top of rink's bundled dataset.
///
/// The CSS lengths, resolutions, and angles. The CSS pixel is a length, while the densities are
/// built on the dimensionless `dot` — the "dot" of CSS's `<resolution>` units — so that density
/// conversions (`dpi` to `dpcm`) and screen sizing (`1920 dot / (300 dpi) -> inch`) both come
/// out right. The CSS names that collide with rink's existing names (`Q`, the quetta prefix
/// symbol; `pt`, the pint; `pc`, the parsec; and the font-relative `em` family, which cannot be
/// a static unit at all) are spelled out or left out.
const EXTRA_UNITS: &str = r#"
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

?? The CSS quarter-millimeter, the default font-size unit of Japanese
?? professional typesetting. The CSS name `Q` is the quetta prefix
?? symbol, so the unit is spelled out.
quartermm                0.25 mm

?? The CSS angle unit. Rink spells it `grade`/`gon`; naming `grad` also
?? stops it from parsing as gram*radian.
grad                     grade

!endcategory
"#;

/// The `.r` command.
const RINK: CommandSpec = CommandSpec::new(
    ".r",
    "Evaluate a calculation with unit and currency conversions",
);

/// Calculator plugin using rink-rs.
pub struct Rink {
    /// The rink context, shared with the currency refresh task, which swaps in a rebuilt
    /// context after fetching new data.
    ctx: Arc<Mutex<RinkContext>>,
    /// The fetched live currency data, refreshed at the TTL and used to rebuild the context.
    currency: Arc<TtlCache<String>>,
    /// The HTTP client used to fetch currency data; `None` when the `currency` setting is
    /// disabled, leaving currency queries to answer with rink's missing-dependencies error.
    client: Option<reqwest::Client>,
    /// The URL the live currency dataset is fetched from.
    currency_url: String,
}

/// Settings for the rink plugin, from its `[plugins.rink]` configuration section.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    /// Whether to fetch live currency (fiat and Bitcoin) data.
    pub currency: bool,
    /// How long the fetched currency data stays valid before being refreshed.
    #[serde(with = "humantime_serde")]
    pub currency_ttl: Duration,
    /// The URL the live currency dataset is fetched from.
    pub currency_url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            currency: true,
            currency_ttl: CURRENCY_TTL,
            currency_url: CURRENCY_URL.to_string(),
        }
    }
}

#[async_trait]
impl Plugin<Context> for Rink {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Rink, ZetaError> {
        subscriptions.command(RINK);

        let client = if settings.currency {
            Some(
                http::builder(&ctx.config.http)
                    .build()
                    .map_err(|error| plugin_err(RequestError::from(error)))?,
            )
        } else {
            None
        };

        Ok(Rink {
            ctx: Arc::new(Mutex::new(build_context(None, None)?)),
            currency: Arc::new(TtlCache::new(settings.currency_ttl)),
            client,
            currency_url: settings.currency_url.clone(),
        })
    }

    async fn loaded(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        self.start_currency_refresh();

        Ok(())
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let message = self.eval_and_format(command.args()).await;

        client.send_privmsg(command.channel(), message)?;

        Ok(())
    }
}

impl Rink {
    /// Starts the background task that refreshes the currency data at the TTL.
    ///
    /// The task fetches on its first tick and again whenever the cached data expires, rebuilding
    /// the rink context with the new rates. Requires the `currency` setting to have created an
    /// HTTP client.
    fn start_currency_refresh(&self) {
        let Some(client) = self.client.clone() else {
            return;
        };

        let (ctx, currency, url) = (
            Arc::clone(&self.ctx),
            Arc::clone(&self.currency),
            self.currency_url.clone(),
        );

        tokio::spawn(
            async move {
                debug!("starting currency refresh task");

                let mut interval = tokio::time::interval(CURRENCY_CHECK_INTERVAL);
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

                loop {
                    interval.tick().await;

                    let refreshed = currency
                        .refresh(|| refresh_currency(&client, &url, &ctx))
                        .await;

                    if let Err(error) = refreshed {
                        warn!(%error, "could not refresh currency data");
                    }
                }
            }
            .instrument(tracing::info_span!("currency_refresh")),
        );
    }

    /// Evaluates `line` and renders the result or error as a one-line IRC reply.
    ///
    /// A query that runs into missing currency data triggers one inline fetch attempt and a
    /// re-evaluation, mirroring upstream rink's REPL, which fetches on demand in the same
    /// situation. A failed refresh is logged and the missing-dependencies error is answered
    /// instead.
    async fn eval_and_format(&self, line: &str) -> String {
        // Rink's results hold `Rc`s and are not `Send`, so each one is rendered before any
        // await: everything but the missing-dependencies retry returns from this match, and
        // the retry re-evaluates the same line below instead of keeping the first result
        // alive across the fetch.
        match self.eval(line) {
            Ok(reply) => return notice(render(&reply)),
            Err(QueryError::MissingDeps(_)) if self.client.is_some() => {}
            Err(error) => return notice(render(&error)),
        }

        // Currency data is missing: one fetch attempt, then a re-evaluation, mirroring
        // upstream rink's REPL, which fetches on demand in the same situation.
        self.refresh_currency_now().await;

        match self.eval(line) {
            Ok(reply) => notice(render(&reply)),
            Err(error) => notice(render(&error)),
        }
    }

    /// Evaluates `line` against the current context.
    fn eval(&self, line: &str) -> Result<QueryReply, QueryError> {
        let mut ctx = crate::sync::lock(&self.ctx);

        rink_core::eval(&mut ctx, line)
    }

    /// Fetches and loads currency data regardless of the cache's freshness.
    ///
    /// Used when a query hit missing dependencies: the cached entry, if any, carries data the
    /// current context evidently lacks. Failures are logged and the next query retries.
    async fn refresh_currency_now(&self) {
        let Some(client) = &self.client else {
            return;
        };

        let refreshed = self
            .currency
            .force_refresh(|| refresh_currency(client, &self.currency_url, &self.ctx))
            .await;

        if let Err(error) = refreshed {
            warn!(%error, "could not refresh currency data");
        }
    }
}

/// Fetches the live currency dataset and swaps a rebuilt context into `ctx`.
///
/// The previous result (`ans`) is carried into the new context, since a query may reference it
/// across a refresh. Returns the fetched body so the caller's cache tracks the data the context
/// was built with; a failed load leaves the context untouched.
///
/// # Errors
///
/// Returns a [`ZetaError`] when the fetch fails, the response carries an error status, or the
/// fetched data cannot be loaded.
async fn refresh_currency(
    client: &reqwest::Client,
    url: &str,
    ctx: &Mutex<RinkContext>,
) -> Result<String, ZetaError> {
    let response = http::send(client.get(url)).await.map_err(plugin_err)?;
    let status = response.status();

    let body = http::text(response).await.map_err(plugin_err)?;

    if !status.is_success() {
        return Err(plugin_err(std::io::Error::other(format!(
            "the currency dataset returned status {status}"
        ))));
    }

    let previous = crate::sync::lock(ctx).previous_result.clone();
    let next = build_context(Some(&body), previous)?;

    *crate::sync::lock(ctx) = next;
    debug!("loaded live currency data into the rink context");

    Ok(body)
}

/// Builds a rink context from the bundled dataset, optional live currency data, and the extra
/// unit definitions.
///
/// Evaluation keeps saving results so `ans` resolves, and `previous` carries the result across
/// context rebuilds.
///
/// # Errors
///
/// Returns a [`ZetaError`] when the bundled dataset, the currency data, or the extra
/// definitions cannot be loaded.
fn build_context(currency: Option<&str>, previous: Option<Value>) -> Result<RinkContext, ZetaError> {
    let mut ctx =
        rink_core::simple_context().map_err(|error| plugin_err(std::io::Error::other(error)))?;
    ctx.save_previous_result = true;
    ctx.previous_result = previous;

    let currency_units = rink_core::CURRENCY_FILE.expect("bundle-files feature to be enabled");
    ctx.load_currency(currency, currency_units)
        .map_err(|error| plugin_err(std::io::Error::other(error)))?;

    ctx.load_definitions(EXTRA_UNITS)
        .map_err(|error| plugin_err(std::io::Error::other(error)))?;

    Ok(ctx)
}

/// Renders `obj`'s markup spans as a one-line IRC reply.
///
/// The scaffolding stays cyan (inherited from the reply prefix), while the values — numbers,
/// datetimes, and the user's echoed input in errors — have their color reset to the default to
/// stand out, and error text renders in red. This mirrors the token-to-color mapping of rink's
/// own IRC frontend, inverted into the house scheme: rink colors unit names and leaves numbers
/// plain, while the house colors the text and resets the values.
fn render<'a, T: TokenFmt<'a>>(obj: &'a T) -> String {
    let mut out = String::new();

    render_spans(&mut out, &obj.to_spans());

    collapse_whitespace(&out)
}

/// Writes `spans` into `out`, styled per their markup hints.
fn render_spans(out: &mut String, spans: &[Span<'_>]) {
    for span in spans {
        match span {
            Span::Content { text, token } => match token {
                FmtToken::Number | FmtToken::DateTime | FmtToken::UserInput => {
                    let _ = write!(out, "{RESET}{text}{COLOR}");
                }
                FmtToken::Error => {
                    let _ = write!(out, "{ERROR_COLOR}{text}{COLOR}");
                }
                _ => out.push_str(text),
            },
            Span::Child(child) => render_spans(out, &child.to_spans()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HttpConfig;
    use zeta_test_support::{
        settings_tests,
        wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        },
    };

    /// A minimal live currency dataset: one unit referencing the bundled euro.
    const TEST_CURRENCY_DATA: &str = r#"[
        {
            "name": "USD",
            "doc": null,
            "category": "currencies",
            "type": "unit",
            "expr": "2 EUR"
        }
    ]"#;

    /// Builds a plugin with a bundled-only context, with currency fetching enabled and pointed
    /// at `url` when given.
    fn test_plugin(url: Option<String>) -> Rink {
        Rink {
            ctx: Arc::new(Mutex::new(build_context(None, None).unwrap())),
            currency: Arc::new(TtlCache::new(Duration::from_mins(10))),
            client: Some(http::build_client(&HttpConfig::default())),
            currency_url: url.unwrap_or_else(|| CURRENCY_URL.to_string()),
        }
    }

    /// Builds a plugin with currency fetching disabled.
    fn offline_plugin() -> Rink {
        Rink {
            ctx: Arc::new(Mutex::new(build_context(None, None).unwrap())),
            currency: Arc::new(TtlCache::new(Duration::from_mins(10))),
            client: None,
            currency_url: CURRENCY_URL.to_string(),
        }
    }

    /// Starts a wiremock server serving `TEST_CURRENCY_DATA` at the currency path.
    async fn currency_server(status: u16) -> MockServer {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/data/currency.json"))
            .respond_with(ResponseTemplate::new(status).set_body_string(TEST_CURRENCY_DATA))
            .mount(&server)
            .await;

        server
    }

    #[tokio::test]
    async fn numeric_results_render_the_value_in_the_default_color() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("4 m").await,
            "\x0310> \x0f4\x0310 meter (length)"
        );
    }

    #[tokio::test]
    async fn errors_render_in_red_with_the_user_input_reset() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("wronginput").await,
            "\x0310> \x034No such unit \x0310\x0fwronginput\x0310"
        );
    }

    #[tokio::test]
    async fn ans_references_the_previous_result() {
        let rink = offline_plugin();

        rink.eval_and_format("4 m").await;

        assert_eq!(
            rink.eval_and_format("ans * 2").await,
            "\x0310> \x0f8\x0310 meter (length)"
        );
    }

    #[tokio::test]
    async fn the_extra_units_load_and_resolve() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("300 px -> mm").await,
            "\x0310> \x0f79.375\x0310 millimeter (length)"
        );
        assert_eq!(
            rink.eval_and_format("90 grad -> degree").await,
            "\x0310> \x0f81\x0310 degree (angle)"
        );
    }

    #[tokio::test]
    async fn css_densities_convert_between_each_other() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("300 dpi -> dpcm").await,
            "\x0310> \x0f15000/127\x0310, approx. \x0f118.1102\x0310 dpcm (m^-1)"
        );
        assert_eq!(
            rink.eval_and_format("1 dppx -> dpi").await,
            "\x0310> \x0f96\x0310 dpi (m^-1)"
        );
    }

    #[tokio::test]
    async fn currency_data_is_loaded_and_used_for_conversions() {
        let server = currency_server(200).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));

        rink.refresh_currency_now().await;

        assert_eq!(
            rink.eval_and_format("1 USD").await,
            "\x0310> \x0f2\x0310 euro (money)"
        );
    }

    #[tokio::test]
    async fn a_failed_fetch_answers_missing_dependencies() {
        let server = currency_server(500).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));

        assert_eq!(
            rink.eval_and_format("1 USD").await,
            "\x0310> Missing dependencies: USD"
        );
    }

    #[tokio::test]
    async fn disabled_currency_answers_missing_dependencies_without_fetching() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("1 USD").await,
            "\x0310> Missing dependencies: USD"
        );
    }

    #[tokio::test]
    #[ignore = "needs network access"]
    async fn the_live_dataset_serves_fiat_and_bitcoin_queries() {
        let rink = test_plugin(None);

        rink.refresh_currency_now().await;

        let fiat = rink.eval_and_format("100 USD to EUR").await;
        assert!(fiat.contains("euro (money)"), "{fiat}");

        let bitcoin = rink.eval_and_format("1 BTC to USD").await;
        assert!(bitcoin.contains("USD (money)"), "{bitcoin}");
    }

    #[tokio::test]
    async fn the_plugin_registers_its_command() {
        let mut subscriptions = Subscriptions::new();

        <Rink as Plugin<Context>>::new(&Context::for_tests(), &Settings::default(), &mut subscriptions)
            .unwrap();

        assert!(subscriptions.commands().contains(&RINK));
    }

    settings_tests! {
        Settings,
        settings,
        default: {
            assert!(settings.currency);
            assert_eq!(settings.currency_ttl, Duration::from_hours(1));
            assert_eq!(settings.currency_url, CURRENCY_URL);
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
}
