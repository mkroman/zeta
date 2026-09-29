//! Evaluates calculations with unit, currency, and cryptocurrency conversions through rink.
//!
//! The `.r <expression>` command evaluates the whole argument as a rink expression and replies
//! with the one-line result as a notice — arithmetic, physical unit conversions, and, with live
//! currency data loaded, fiat currency, Bitcoin, and cryptocurrency lookups. The
//! `.c <expression>` command does the same but case-corrects unit tokens rink cannot resolve,
//! so lowercase abbreviations like `10 usd to dkk` evaluate like `10 USD to DKK`. Evaluation
//! errors are reported inline as `> Error: <message>`.
//!
//! Replies are styled like the other plugins: the cyan `> ` marker and scaffolding, with the
//! values — numbers, datetimes, property names, echoed input in errors, category labels, and
//! the unit expressions that follow numbers — reset to the default color to stand out. Rink's
//! output is rendered from its markup spans rather than its plain text form, so the styling can
//! follow the structure of the answer, and control codes are emitted only where the style
//! actually changes — never duplicated, never dangling at the end of the message.
//!
//! A rink context is built once at plugin initialization: the bundled unit definitions, the
//! static currency units, and a small set of extra definitions (CSS lengths, resolutions,
//! angles, and the cryptocurrency subunits) loaded on top. Evaluation calls are serialized
//! through it, and a failed context build aborts plugin initialization.
//!
//! When the `currency` setting is enabled (the default), live currency data is fetched from
//! rink's dataset — fiat rates from the European Central Bank and Bitcoin from blockchain.info
//! — and, when the coinmarketcap plugin feature is compiled in and the plugin shares its
//! client, the ranked cryptocurrency table from the CoinMarketCap API; without that feature
//! the cryptocurrency units stay pending. The `crypto` setting turns the cryptocurrency
//! loading off. Both datasets are rebuilt into the context by a background task started when
//! the plugin loads, once per `currency_ttl`. A query that runs into missing currency data
//! re-evaluates after an inline refresh — mirroring upstream rink's REPL, which fetches on
//! demand in the same situation — but that refresh only runs when the cached dataset is
//! missing, stale, or still lacks a requested cryptocurrency table, so an unresolvable
//! dependency does not fetch once per query; failed fetches are logged and leave the last
//! known rates in place. The dataset plumbing lives in the `currency` submodule.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rink_core::ast::{Conversion, Expr, Query};
use rink_core::output::fmt::{FmtToken, Span, TokenFmt};
use rink_core::output::{QueryError, QueryReply};
use rink_core::parsing::text_query::{parse_query, TokenIterator};
use rink_core::Context as RinkContext;
use serde::{Deserialize, Serialize};
use tokio::time::MissedTickBehavior;
use tracing::{Instrument, debug, warn};

mod currency;
use currency::{CurrencyData, build_context, refresh_data};

use crate::cache::TtlCache;
#[cfg(feature = "plugin-coinmarketcap")]
use crate::plugin::coinmarketcap::client;
use crate::{
    error::RequestError,
    plugin::prelude::*,
    utils::collapse_whitespace,
};

const RINK: CommandSpec = CommandSpec::new(
    ".r",
    "Evaluate a calculation with unit and currency conversions",
);

/// The `.c` command.
const CLASSIFY: CommandSpec = CommandSpec::new(
    ".c",
    "Evaluate a calculation, correcting unit casing",
);

/// Calculator plugin using rink-rs.
pub struct Rink {
    /// The rink context, shared with the currency refresh task, which swaps in a rebuilt
    /// context after fetching new data.
    ctx: Arc<Mutex<RinkContext>>,
    /// The fetched live currency data, refreshed at the TTL and used to rebuild the context.
    currency: Arc<TtlCache<CurrencyData>>,
    /// The HTTP client used to fetch currency data; `None` when the `currency` setting is
    /// disabled, leaving currency queries to answer with rink's missing-dependencies error.
    client: Option<reqwest::Client>,
    /// The keyed CoinMarketCap client shared by the coinmarketcap plugin, used to fetch the
    /// cryptocurrency listings; `None` when the coinmarketcap plugin is not loaded.
    #[cfg(feature = "plugin-coinmarketcap")]
    cmc: Option<Arc<client::Client>>,
    /// The URL the live currency dataset is fetched from.
    currency_url: String,
    /// Whether to load live cryptocurrency units; effective only when the coinmarketcap plugin
    /// has shared its client.
    #[cfg(feature = "plugin-coinmarketcap")]
    crypto_enabled: bool,
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
    /// Whether to load live cryptocurrency units through the coinmarketcap plugin.
    ///
    /// Effective only with the coinmarketcap plugin feature compiled in; without it the
    /// cryptocurrency units stay pending.
    #[cfg(feature = "plugin-coinmarketcap")]
    pub crypto: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            currency: true,
            currency_ttl: currency::CURRENCY_TTL,
            currency_url: currency::CURRENCY_URL.to_string(),
            #[cfg(feature = "plugin-coinmarketcap")]
            crypto: true,
        }
    }
}

#[async_trait]
impl Plugin<Context> for Rink {
    type Settings = Settings;

    fn new(ctx: &Context, settings: &Settings, subscriptions: &mut Subscriptions) -> Result<Rink, ZetaError> {
        subscriptions.command(RINK).command(CLASSIFY);

        let client = if settings.currency {
            Some(
                crate::http::builder(&ctx.config.http)
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
            #[cfg(feature = "plugin-coinmarketcap")]
            cmc: None,
            currency_url: settings.currency_url.clone(),
            #[cfg(feature = "plugin-coinmarketcap")]
            crypto_enabled: settings.crypto,
        })
    }

    async fn loaded(&mut self, #[allow(unused)] ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        // The coinmarketcap plugin publishes its keyed client for other plugins to reuse.
        #[cfg(feature = "plugin-coinmarketcap")]
        {
            self.cmc = ctx.shared.get::<client::Client>();
        }

        self.start_currency_refresh();

        Ok(())
    }

    async fn handle_command(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let classify = command.spec == CLASSIFY;
        let message = self.eval_and_format(command.args(), classify).await;

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

        #[cfg(feature = "plugin-coinmarketcap")]
        let (cmc, crypto_enabled) = (self.cmc.clone(), self.crypto_enabled);

        tokio::spawn(
            async move {
                debug!("starting currency refresh task");

                let mut interval = tokio::time::interval(currency::CURRENCY_CHECK_INTERVAL);
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

                loop {
                    interval.tick().await;

                    #[cfg(feature = "plugin-coinmarketcap")]
                    let refreshed = currency
                        .refresh(|| {
                            refresh_data(
                                &client,
                                &currency,
                                cmc.as_ref(),
                                crypto_enabled,
                                &url,
                                &ctx,
                            )
                        })
                        .await;

                    #[cfg(not(feature = "plugin-coinmarketcap"))]
                    let refreshed =
                        currency.refresh(|| refresh_data(&client, &url, &ctx)).await;

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
    /// When `classify` is set, unit tokens rink cannot resolve are case-corrected against the
    /// registry first, so `.c 10 usd to dkk` evaluates like `10 USD to DKK`.
    ///
    /// A query that runs into missing currency data triggers an inline refresh and a
    /// re-evaluation, mirroring upstream rink's REPL, which fetches on demand in the same
    /// situation. The refresh is only attempted when it can actually change the outcome —
    /// the cached dataset is missing, stale, or lacks a requested cryptocurrency table — so
    /// a dependency that stays unresolvable does not fetch on every query. A failed refresh
    /// is logged and the missing-dependencies error is answered instead.
    async fn eval_and_format(&self, line: &str, classify: bool) -> String {
        let line = if classify {
            self.classify(line)
        } else {
            line.to_owned()
        };

        // Rink's results hold `Rc`s and are not `Send`, so each one is rendered before any
        // await: everything but the missing-dependencies retry returns from this match, and
        // the retry re-evaluates the same line below instead of keeping the first result
        // alive across the fetch.
        match self.eval(&line) {
            Ok(reply) => return notice(render_reply(&reply)),
            Err(QueryError::MissingDeps(_)) if self.client.is_some() => {}
            Err(error) => return notice(error_message(&error)),
        }

        // Currency data is missing: refresh what can still change the outcome, then
        // re-evaluate, mirroring upstream rink's REPL, which fetches on demand in the same
        // situation.
        self.ensure_currency_cached().await;

        match self.eval(&line) {
            Ok(reply) => notice(render_reply(&reply)),
            Err(error) => notice(error_message(&error)),
        }
    }

    /// Case-corrects the unit tokens in `line` that rink cannot resolve.
    ///
    /// The correction runs on rink's parsed query rather than the input string, so number-and-
    /// unit words like `10usd` are handled by rink's own tokenizer. Each `Expr::Unit` name that
    /// does not resolve is matched case-insensitively against the registry's names — defined
    /// units, substances, base units and pending dependencies — and replaced when exactly one
    /// candidate exists. Queries that end up untouched, including ones the parser cannot make
    /// sense of, are returned unchanged.
    fn classify(&self, line: &str) -> String {
        let ctx = crate::sync::lock(&self.ctx);
        let mut iter = TokenIterator::new(line.trim()).peekable();
        let mut query = parse_query(&mut iter);

        if fix_unit_casing(&ctx, &mut query) {
            query.spans_to_string()
        } else {
            line.to_owned()
        }
    }

    /// Evaluates `line` against the current context.
    fn eval(&self, line: &str) -> Result<QueryReply, QueryError> {
        let mut ctx = crate::sync::lock(&self.ctx);

        rink_core::eval(&mut ctx, line)
    }

    /// Fetches and loads currency data when a refetch can still change the outcome.
    ///
    /// Used when a query hit missing dependencies. Like the coinmarketcap plugin's
    /// `ensure_coins_cached`, the refresh only runs when the cache is missing or stale, so a
    /// dependency that stays unresolvable does not fetch once per query. The exception is a
    /// requested-but-absent cryptocurrency table: a listings outage is retried immediately
    /// instead of waiting out a whole [`currency_ttl`], since that state is exactly what a
    /// refetch can fill. Failures are logged and the next qualifying query retries.
    ///
    /// [`currency_ttl`]: Settings::currency_ttl
    async fn ensure_currency_cached(&self) {
        let Some(client) = &self.client else {
            return;
        };

        #[cfg(feature = "plugin-coinmarketcap")]
        let refreshed = {
            let fetch = || {
                refresh_data(
                    client,
                    &self.currency,
                    self.cmc.as_ref(),
                    self.crypto_enabled,
                    &self.currency_url,
                    &self.ctx,
                )
            };

            // A missing or stale cache is fetched through `refresh`; only a requested table
            // that is still absent forces the fetch past a fresh entry.
            if self.crypto_table_pending() {
                self.currency.force_refresh(fetch).await.map(|_| ())
            } else {
                self.currency.refresh(fetch).await
            }
        };

        #[cfg(not(feature = "plugin-coinmarketcap"))]
        let refreshed = self
            .currency
            .refresh(|| refresh_data(client, &self.currency_url, &self.ctx))
            .await;

        if let Err(error) = refreshed {
            warn!(%error, "could not refresh currency data");
        }
    }

    /// Returns whether a requested cryptocurrency table is absent from the cached dataset.
    ///
    /// `true` when the `crypto` setting is on and the coinmarketcap plugin shared its client,
    /// but nothing is cached yet or the cached entry carries no listings. Only in that state
    /// does an unconditional fetch pay off: the fiat data may be fresh while the table a query
    /// needs is still missing. In every other case the cache already holds what the context
    /// was built from, and re-fetching cannot resolve a pending dependency.
    #[cfg(feature = "plugin-coinmarketcap")]
    fn crypto_table_pending(&self) -> bool {
        self.crypto_enabled
            && self.cmc.is_some()
            && self.currency
                .read(|data| data.is_none_or(|entry| entry.crypto.is_none()))
    }
}

/// Rewrites every `Expr::Unit` name in `query` that rink cannot resolve into its
/// case-corrected spelling, when exactly one registry name matches case-insensitively.
///
/// Names are drawn from the registry's defined units, substances, base units, their long forms
/// and pending dependencies — so lowercase ISO codes like `usd` correct to `USD` both while the
/// currency data is missing and once it is loaded. Returns whether anything was rewritten; an
/// untouched query must be evaluated verbatim, since re-rendering it could shift a parse that
/// only rink's own error handling can report correctly.
fn fix_unit_casing(ctx: &RinkContext, query: &mut Query) -> bool {
    fn fix_expr(ctx: &RinkContext, expr: &mut Expr, fixed: &mut bool) {
        match expr {
            Expr::Unit { name } => {
                if ctx.lookup(name).is_none() {
                    let mut candidates: Vec<String> = ctx
                        .registry
                        .units
                        .keys()
                        .cloned()
                        .chain(ctx.registry.missing_deps.keys().cloned())
                        .chain(ctx.registry.substances.keys().cloned())
                        .chain(ctx.registry.base_unit_long_names.keys().cloned())
                        .chain(
                            ctx.registry
                                .base_units
                                .iter()
                                .map(|unit| unit.id.to_string()),
                        )
                        .filter(|candidate| candidate.eq_ignore_ascii_case(name))
                        .collect();
                    candidates.sort();
                    candidates.dedup();

                    if let [fixed_name] = &candidates[..] {
                        name.clone_from(fixed_name);
                        *fixed = true;
                    }
                }
            }
            Expr::BinOp(binop) => {
                fix_expr(ctx, &mut binop.left, fixed);
                fix_expr(ctx, &mut binop.right, fixed);
            }
            Expr::UnaryOp(unaryop) => fix_expr(ctx, &mut unaryop.expr, fixed),
            Expr::Mul { exprs } => {
                for expr in exprs {
                    fix_expr(ctx, expr, fixed);
                }
            }
            Expr::Of { expr, .. } => fix_expr(ctx, expr, fixed),
            Expr::Call { args, .. } => {
                for arg in args {
                    fix_expr(ctx, arg, fixed);
                }
            }
            Expr::Quote { .. } | Expr::Const { .. } | Expr::Date { .. } | Expr::Error { .. } => {}
        }
    }

    let mut fixed = false;

    let (top, conversion) = match query {
        Query::Expr(expr) | Query::Factorize(expr) | Query::UnitsFor(expr) => (expr, None),
        Query::Convert(top, conversion, ..) => (top, Some(conversion)),
        Query::Search(_) | Query::Error(_) => return fixed,
    };

    fix_expr(ctx, top, &mut fixed);

    if let Some(Conversion::Expr(bottom)) = conversion {
        fix_expr(ctx, bottom, &mut fixed);
    }

    fixed
}

/// The styles rink's output renders in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    /// The scaffolding, in the reply cyan.
    Text,
    /// A value, in the default color.
    Value,
}

impl Style {
    /// Returns the control code that enters this style.
    const fn code(self) -> &'static str {
        match self {
            Style::Text => COLOR,
            Style::Value => RESET,
        }
    }
}

/// Renders a successful evaluation's markup spans as a one-line IRC reply.
fn render_reply(reply: &QueryReply) -> String {
    render(reply, false)
}

/// Renders a failed evaluation's markup spans as a one-line IRC reply.
///
/// In error replies every unit renders as a value: the units there are the answer itself — a
/// not-found suggestion or a missing dependency — rather than scaffolding.
fn render_error(error: &QueryError) -> String {
    render(error, true)
}

/// Formats a failed evaluation as the labeled `Error: <message>` reply body.
fn error_message(error: &QueryError) -> String {
    format!("Error: {}", render_error(error))
}

/// Renders `obj`'s markup spans as a one-line IRC reply.
///
/// The scaffolding stays cyan (inherited from the reply prefix), while the values — numbers,
/// datetimes, property names, the user's echoed input in errors, category labels, and the unit
/// expressions that follow numbers — have their color reset to the default to stand out. This
/// mirrors the token-to-color mapping of rink's own IRC frontend, inverted into the house
/// scheme: rink colors unit names and leaves numbers plain, while the house colors the text and
/// resets the values.
fn render<'a, T: TokenFmt<'a>>(obj: &'a T, error: bool) -> String {
    let mut out = String::new();
    let mut style = Style::Text;

    render_spans(&mut out, &mut style, error, &obj.to_spans());

    collapse_whitespace(&out)
}

/// Writes `spans` into `out`, styled per their markup hints.
///
/// A style transition emits exactly one control code, and only when text follows it: never
/// duplicated, never dangling at the end of the message. Whitespace is transparent — it keeps
/// whatever run it lands in and never triggers a transition of its own, so a value group like
/// `224.256 DKK` stays one reset run.
fn render_spans(out: &mut String, style: &mut Style, error: bool, spans: &[Span<'_>]) {
    for span in spans {
        match span {
            Span::Content { text, token } => {
                if text.is_empty() {
                    continue;
                }

                if text.chars().all(char::is_whitespace) {
                    out.push_str(text);
                    continue;
                }

                let target = match token {
                    FmtToken::Number
                    | FmtToken::DateTime
                    | FmtToken::UserInput
                    | FmtToken::PropName
                    | FmtToken::Quantity => Style::Value,
                    FmtToken::Unit | FmtToken::Pow if *style == Style::Value || error => {
                        Style::Value
                    }
                    _ => Style::Text,
                };

                if *style != target {
                    out.push_str(target.code());
                    *style = target;
                }

                out.push_str(text);
            }
            Span::Child(child) => render_spans(out, style, error, &child.to_spans()),
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

    /// A minimal live currency dataset: USD, DKK and JPY against the bundled euro, plus a
    /// Bitcoin substance so the subunit expressions resolve.
    const TEST_CURRENCY_DATA: &str = r#"[
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
    #[cfg(feature = "plugin-coinmarketcap")]
    const TEST_LISTINGS_DATA: &str = r#"{
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
    fn test_plugin(url: Option<String>) -> Rink {
        Rink {
            ctx: Arc::new(Mutex::new(build_context(None, None).unwrap())),
            currency: Arc::new(TtlCache::new(Duration::from_mins(10))),
            client: Some(crate::http::build_client(&HttpConfig::default())),
            #[cfg(feature = "plugin-coinmarketcap")]
            cmc: None,
            currency_url: url.unwrap_or_else(|| currency::CURRENCY_URL.to_string()),
            #[cfg(feature = "plugin-coinmarketcap")]
            crypto_enabled: true,
        }
    }

    /// Builds a plugin with currency fetching disabled.
    fn offline_plugin() -> Rink {
        Rink {
            ctx: Arc::new(Mutex::new(build_context(None, None).unwrap())),
            currency: Arc::new(TtlCache::new(Duration::from_mins(10))),
            client: None,
            #[cfg(feature = "plugin-coinmarketcap")]
            cmc: None,
            currency_url: currency::CURRENCY_URL.to_string(),
            #[cfg(feature = "plugin-coinmarketcap")]
            crypto_enabled: true,
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

    /// Starts a wiremock server serving `TEST_LISTINGS_DATA` at the CoinMarketCap listings
    /// path.
    #[cfg(feature = "plugin-coinmarketcap")]
    async fn listings_server(status: u16) -> MockServer {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/v3/cryptocurrency/listings/latest"))
            .respond_with(ResponseTemplate::new(status).set_body_string(TEST_LISTINGS_DATA))
            .mount(&server)
            .await;

        server
    }

    /// Builds a plugin whose CoinMarketCap client issues requests against the given listings
    /// server.
    #[cfg(feature = "plugin-coinmarketcap")]
    async fn plugin_with_listings(status: u16) -> Rink {
        let fiat_server = currency_server(200).await;
        let listings_server = listings_server(status).await;

        let mut rink = test_plugin(Some(format!("{}/data/currency.json", fiat_server.uri())));
        rink.cmc = Some(Arc::new(client::Client::for_base(
            "test-api-key",
            &HttpConfig::default(),
            &listings_server.uri(),
        )));

        rink
    }

    #[tokio::test]
    async fn numeric_results_render_the_value_in_the_default_color() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("4 m", false).await,
            "\x0310> \x0f4 meter \x0310(\x0flength\x0310)"
        );
    }

    #[tokio::test]
    async fn errors_render_behind_an_error_label_with_the_input_reset() {
        let rink = offline_plugin();

        // A query without a close match carries no suggestion.
        assert_eq!(
            rink.eval_and_format("wronginput", false).await,
            "\x0310> Error: No such unit \x0fwronginput"
        );
    }

    #[tokio::test]
    async fn ans_references_the_previous_result() {
        let rink = offline_plugin();

        rink.eval_and_format("4 m", false).await;

        assert_eq!(
            rink.eval_and_format("ans * 2", false).await,
            "\x0310> \x0f8 meter \x0310(\x0flength\x0310)"
        );
    }

    #[tokio::test]
    async fn substances_render_every_property_and_label_as_a_value() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("egg", false).await,
            concat!(
                "\x0310> egg: USA large egg. ",
                "\x0fmass_shelled\x0310 = \x0f50 gram \x0310(\x0fmass\x0310); ",
                "\x0fmass_white\x0310 = \x0f30 gram \x0310(\x0fmass\x0310); ",
                "\x0fmass_yolk\x0310 = \x0f18.6 gram \x0310(\x0fmass\x0310); ",
                "\x0fvolume\x0310 = approx. \x0f46824.75 millimeter^3 \x0310(\x0fvolume\x0310); ",
                "\x0fvolume_white\x0310 = approx. \x0f29573.52 millimeter^3 \x0310(\x0fvolume\x0310); ",
                "\x0fvolume_yolk\x0310 = approx. \x0f17251.22 millimeter^3 \x0310(\x0fvolume\x0310)",
            )
        );
    }

    #[tokio::test]
    async fn a_unit_span_following_a_number_stays_inside_the_value_run() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("helium", false).await,
            concat!(
                "\x0310> helium: ",
                "\x0fatomic_number\x0310 = \x0f2 \x0310(\x0fdimensionless\x0310); ",
                "\x0fmolar_mass\x0310 = \x0f4.002602 gram / mole \x0310(\x0fmolar_mass\x0310)",
            )
        );
    }

    #[tokio::test]
    async fn the_extra_units_load_and_resolve() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("300 px -> mm", false).await,
            "\x0310> \x0f79.375 millimeter \x0310(\x0flength\x0310)"
        );
        assert_eq!(
            rink.eval_and_format("90 grad -> degree", false).await,
            "\x0310> \x0f81 degree \x0310(\x0fangle\x0310)"
        );
    }

    #[tokio::test]
    async fn css_densities_convert_between_each_other() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("300 dpi -> dpcm", false).await,
            "\x0310> \x0f15000/127\x0310, approx. \x0f118.1102 dpcm \x0310(\x0fm^-1\x0310)"
        );
        assert_eq!(
            rink.eval_and_format("1 dppx -> dpi", false).await,
            "\x0310> \x0f96 dpi \x0310(\x0fm^-1\x0310)"
        );
    }

    #[cfg(feature = "plugin-coinmarketcap")]
    #[tokio::test]
    async fn crypto_units_load_and_convert() {
        let rink = plugin_with_listings(200).await;

        rink.ensure_currency_cached().await;

        assert_eq!(
            rink.eval_and_format("1 ETH to USD", false).await,
            "\x0310> \x0f2650.32 USD \x0310(\x0fmoney\x0310)"
        );

        // The classifier case-corrects lowercase coin abbreviations too.
        assert_eq!(
            rink.eval_and_format("1 eth to usd", true).await,
            "\x0310> \x0f2650.32 USD \x0310(\x0fmoney\x0310)"
        );
    }

    #[cfg(feature = "plugin-coinmarketcap")]
    #[tokio::test]
    async fn synthesis_skips_colliding_and_invalid_symbols() {
        let rink = plugin_with_listings(200).await;

        rink.ensure_currency_cached().await;

        // The fake EUR coin (priced 1.2) must not shadow the bundled euro: 1 EUR is 0.5 USD
        // under the fiat fixture, and the rejected `1INCH` symbol must not break the load.
        assert_eq!(
            rink.eval_and_format("1 EUR to USD", false).await,
            "\x0310> \x0f0.5 USD \x0310(\x0fmoney\x0310)"
        );
    }

    #[cfg(feature = "plugin-coinmarketcap")]
    #[test]
    fn a_duplicate_ticker_keeps_the_whole_context_build_loadable() {
        use crate::plugin::coinmarketcap::model::{Envelope, Listing};

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
            crate::http::json::from_str(text).expect("the fixture should decode");
        let data = CurrencyData {
            fiat: TEST_CURRENCY_DATA.to_string(),
            crypto: parsed.data,
        };

        let ctx = build_context(Some(&data), None)
            .expect("a duplicate ticker must not fail the whole context build");

        // The fiat dataset landed, and the higher-ranked coin of the pair survived.
        assert!(ctx.lookup("USD").is_some(), "the fiat data must still load");
        assert!(ctx.lookup("FOO").is_some(), "the first-ranked coin must load");
    }

    #[cfg(feature = "plugin-coinmarketcap")]
    #[tokio::test]
    async fn crypto_subunits_resolve_against_the_live_coins() {
        let rink = plugin_with_listings(200).await;

        rink.ensure_currency_cached().await;

        // 50_000 satoshis are 0.0005 bitcoin at the fixture's 65 000 USD price.
        assert_eq!(
            rink.eval_and_format("50000 sats to usd", true).await,
            "\x0310> \x0f32.5 USD \x0310(\x0fmoney\x0310)"
        );
        // A gwei stays convertible without being resolvable as fiat.
        let reply = rink.eval_and_format("1 gwei to usd", true).await;
        assert!(reply.contains("USD"), "{reply}");
    }

    #[cfg(feature = "plugin-coinmarketcap")]
    #[tokio::test]
    async fn crypto_disabled_without_the_coinmarketcap_plugin() {
        let mut rink = offline_plugin();
        // No coinmarketcap client was published: the crypto units stay unloaded even though
        // the crypto setting is on.
        rink.crypto_enabled = true;
        rink.cmc = None;

        rink.ensure_currency_cached().await;

        // The ETH unit is not loaded, but the subunits' pending dependency declaration
        // surfaces it in the missing-dependencies error.
        assert_eq!(
            rink.eval_and_format("1 eth to usd", true).await,
            "\x0310> Error: Missing dependencies: \x0fETH"
        );
    }

    #[cfg(feature = "plugin-coinmarketcap")]
    #[tokio::test]
    async fn a_failed_listings_fetch_keeps_the_fiat_path() {
        // The fiat dataset is fine; the listings endpoint is not. The refresh fails as a
        // whole, so the last known context — without crypto — stays in place.
        let rink = plugin_with_listings(500).await;

        rink.ensure_currency_cached().await;

        // The failed listings fetch keeps the crypto units out, but the subunits' pending
        // dependency still surfaces ETH in the error.
        assert_eq!(
            rink.eval_and_format("1 eth to usd", true).await,
            "\x0310> Error: Missing dependencies: \x0fETH"
        );
    }

    #[cfg(feature = "plugin-coinmarketcap")]
    #[tokio::test]
    async fn a_pending_crypto_table_is_retried_inline() {
        let fiat = currency_server(200).await;
        let listings = listings_server(500).await;

        let mut rink = test_plugin(Some(format!("{}/data/currency.json", fiat.uri())));
        rink.cmc = Some(Arc::new(client::Client::for_base(
            "test-api-key",
            &HttpConfig::default(),
            &listings.uri(),
        )));

        rink.ensure_currency_cached().await;
        assert_eq!(listings.received_requests().await.unwrap().len(), 1);

        // The listings fetch failed, so the requested table is absent even though the fiat
        // entry is fresh: unlike an unresolvable dependency, this state a refetch can fix,
        // so the query retries the listings inline instead of waiting out the whole TTL.
        let reply = rink.eval_and_format("1 gwei to usd", true).await;
        assert!(reply.contains("Missing dependencies"), "{reply}");

        assert_eq!(listings.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_failed_fetch_answers_missing_dependencies() {
        let server = currency_server(500).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));

        assert_eq!(
            rink.eval_and_format("1 USD", false).await,
            "\x0310> Error: Missing dependencies: \x0fUSD"
        );
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
            let reply = rink.eval_and_format("1 gwei to usd", true).await;

            assert!(reply.contains("Missing dependencies"), "{reply}");
        }

        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn disabled_currency_answers_missing_dependencies_without_fetching() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("1 USD", false).await,
            "\x0310> Error: Missing dependencies: \x0fUSD"
        );
    }

    #[tokio::test]
    async fn classify_case_corrects_unresolvable_units() {
        let server = currency_server(200).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));

        rink.ensure_currency_cached().await;

        assert_eq!(
            rink.eval_and_format("10 usd to dkk", true).await,
            "\x0310> \x0f149.504 DKK \x0310(\x0fmoney\x0310)"
        );
        assert_eq!(
            rink.eval_and_format("10usd to dkk", true).await,
            "\x0310> \x0f149.504 DKK \x0310(\x0fmoney\x0310)"
        );
        assert_eq!(
            rink.eval_and_format("10 USD to DKK", true).await,
            "\x0310> \x0f149.504 DKK \x0310(\x0fmoney\x0310)"
        );
        assert_eq!(
            rink.eval_and_format("30 eur to jpy", true).await,
            "\x0310> \x0f5115.6 JPY \x0310(\x0fmoney\x0310)"
        );
    }

    #[tokio::test]
    async fn classify_corrects_units_while_the_currency_data_is_missing() {
        let rink = offline_plugin();

        // The corrections land before any evaluation: the reply names the *tracked*
        // dependencies (USD), which only the rewritten `10 USD to DKK` can trigger.
        assert_eq!(
            rink.eval_and_format("10 usd to dkk", true).await,
            "\x0310> Error: Missing dependencies: \x0fUSD"
        );
    }

    #[tokio::test]
    async fn classify_leaves_unclassifiable_tokens_alone() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("50 gram", true).await,
            "\x0310> \x0f50 gram \x0310(\x0fmass\x0310)"
        );
        assert_eq!(
            rink.eval_and_format("banana", true).await,
            "\x0310> Error: No such unit \x0fbanana"
        );
        assert_eq!(
            rink.eval_and_format("2 hours from now", true).await,
            "\x0310> Error: No such unit \x0ffrom\x0310, did you mean \x0ffreon\x0310?"
        );
    }

    #[tokio::test]
    async fn classify_returns_unparseable_queries_unchanged() {
        let rink = offline_plugin();

        assert_eq!(
            rink.eval_and_format("20:00 in tokyo", true).await,
            "\x0310> Error: Expected term, got `:`"
        );
    }

    #[tokio::test]
    async fn classify_evaluates_dates() {
        let rink = offline_plugin();

        // A fixed date keeps the assertions free of the clock and the host timezone.
        let reply = rink.eval_and_format("#2016-08-24#", true).await;
        // The datetime renders as a value, inside the run opened right after the marker.
        assert!(reply.contains("\x0f2016-08-24"), "{reply}");
        // The value run closes right after the datetime's trailing timezone label.
        assert!(reply.contains("]\x0310"), "{reply}");
    }

    #[tokio::test]
    async fn classification_leaves_every_other_expression_form_unchanged() {
        let rink = offline_plugin();

        // Expression forms from rink's manual; classification must not change the outcome of
        // any of them. Clock-dependent forms are excluded: their output legitimately differs
        // between the two evaluations.
        for line in [
            "10.1e2",
            "0x10",
            "0o10",
            "0b10",
            "1_000",
            "1e100",
            "3 4 m 5 s",
            "10 km / 5 m",
            "1|2 m",
            "12 meters + 5 feet",
            "12 ft^2",
            "meter mod foot",
            "1 << 24",
            "0b1010 and 0b1100 to base 2",
            "12 degC -> °F",
            "2000 kcal -> potato = 164 kcal",
            "12 'core' hour / 3 'core' -> minutes",
            "5 millimeters",
            "3 kibibytes",
            "2 quarters",
            "meter -> feet",
            "1000 -> hex",
            "10000 -> base 36",
            "2^128 -> digits",
            "mass of electron -> eng",
            "3 foot -> frac",
            "2^17 seconds -> hour;min;sec",
            "units for power",
            "units power",
            "factorize velocity",
            "search milk",
            "#jan 01, 1970#",
            "#2016-08-24# + 500 weeks",
            "#today#",
            "#apr 1, 2016 12:00:00 +01:00#",
            "milk",
            "gallon milk",
            "egg",
            "egg_shelled of kg egg",
            "gallon gasoline -> btu",
            "sqrt(2)",
            "fac(5)",
            // failures must stay identical too
            "banana",
            "2 hours from now",
            "20:00 in tokyo",
            "1 << meter",
        ] {
            let strict = rink.eval_and_format(line, false).await;
            let smart = rink.eval_and_format(line, true).await;
            assert_eq!(strict, smart, "classification changed {line:?}");
        }
    }

    #[tokio::test]
    #[ignore = "needs network access"]
    async fn the_live_dataset_serves_fiat_and_bitcoin_queries() {
        let rink = test_plugin(None);

        rink.ensure_currency_cached().await;

        let fiat = rink.eval_and_format("100 usd to eur", true).await;
        assert!(fiat.contains("euro") && fiat.contains("money"), "{fiat}");

        let bitcoin = rink.eval_and_format("1 btc to usd", true).await;
        assert!(bitcoin.contains("USD") && bitcoin.contains("money"), "{bitcoin}");
    }

    #[tokio::test]
    async fn the_plugin_registers_its_commands() {
        let mut subscriptions = Subscriptions::new();

        <Rink as Plugin<Context>>::new(&Context::for_tests(), &Settings::default(), &mut subscriptions)
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
}
