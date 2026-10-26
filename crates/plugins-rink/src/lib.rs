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
//! angles, and the cryptocurrency subunits) loaded on top. The context is moved into a
//! dedicated actor task when the plugin loads: evaluation calls are serialized through that
//! task's mailbox, and a failed context build aborts plugin initialization.
//!
//! When the `currency` setting is enabled (the default), live currency data is fetched from
//! rink's dataset — fiat rates from the European Central Bank and Bitcoin from blockchain.info
//! — and, when the coinmarketcap plugin is loaded and shares its
//! client, the ranked cryptocurrency table from the CoinMarketCap API; without that feature
//! the cryptocurrency units stay pending. The `crypto` setting turns the cryptocurrency
//! loading off. Both datasets are rebuilt into a fresh context by a background task started
//! when the plugin loads, once per `currency_ttl`; the task asks the actor to swap it in. A
//! query that runs into missing currency data re-evaluates after an inline refresh — mirroring
//! upstream rink's REPL, which fetches on demand in the same situation — but that refresh only
//! runs when the cached dataset is missing, stale, or still lacks a requested cryptocurrency
//! table, so an unresolvable dependency does not fetch once per query; failed fetches are
//! logged and leave the last known rates in place. The dataset plumbing lives in the `currency`
//! submodule.

use std::sync::Arc;
use std::time::Duration;

use rink_core::Context as RinkContext;
use rink_core::ast::{Conversion, Expr, Query};
use rink_core::output::fmt::{FmtToken, Span, TokenFmt};
use rink_core::output::{QueryError, QueryReply};
use rink_core::parsing::text_query::{TokenIterator, parse_query};
use serde::{Deserialize, Serialize};
use tokio::time::MissedTickBehavior;
use tracing::{Instrument, debug, warn};

mod actor;
mod currency;
use actor::{EvalOutcome, Handle};
use currency::{CurrencyData, build_context, refresh_data};

use coinmarketcap::Client as CmcClient;
use zeta_cache::TtlCache;
use zeta_http::RequestError;
use zeta_plugin::prelude::*;
use zeta_utils::text::collapse_whitespace;

/// Calculator plugin using rink-rs.
pub struct Rink {
    /// The rink engine, moved into the actor task when the plugin loads.
    engine: Option<RinkContext>,
    /// The actor task owning the context and serializing evaluations through its mailbox.
    actor: Option<Handle>,
    /// The fetched live currency data, refreshed at the TTL and used to rebuild the context.
    currency: Arc<TtlCache<CurrencyData>>,
    /// The background task refreshing the currency data, stopped on shutdown.
    refresh_task: Option<tokio::task::JoinHandle<()>>,
    /// The HTTP client used to fetch currency data; `None` when the `currency` setting is
    /// disabled, leaving currency queries to answer with rink's missing-dependencies error.
    client: Option<reqwest::Client>,
    /// The keyed CoinMarketCap client shared by the coinmarketcap plugin, used to fetch the
    /// cryptocurrency listings; `None` when the coinmarketcap plugin is not loaded.
    cmc: Option<Arc<CmcClient>>,
    /// The URL the live currency dataset is fetched from.
    currency_url: String,
    /// Whether to load live cryptocurrency units; effective only when the coinmarketcap plugin
    /// has shared its client.
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
    /// Effective only when the plugin has shared its client; without it the cryptocurrency
    /// units stay pending.
    pub crypto: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            currency: true,
            currency_ttl: currency::CURRENCY_TTL,
            currency_url: currency::CURRENCY_URL.to_string(),
            crypto: true,
        }
    }
}

#[plugin(name = "rink", settings = Settings)]
impl Rink {
    fn new(ctx: &Context, settings: &Settings) -> Result<Self, ZetaError> {
        let client = if settings.currency {
            Some(
                zeta_http::builder(&ctx.config.http)
                    .build()
                    .map_err(|error| plugin_err(RequestError::from(error)))?,
            )
        } else {
            None
        };

        Ok(Self {
            engine: Some(build_context(None, None)?),
            actor: None,
            currency: Arc::new(TtlCache::new(settings.currency_ttl)),
            refresh_task: None,
            client,
            cmc: None,
            currency_url: settings.currency_url.clone(),
            crypto_enabled: settings.crypto,
        })
    }

    #[handler(loaded)]
    async fn loaded(&mut self, ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        // The coinmarketcap plugin publishes its keyed client for other plugins to reuse.
        self.cmc = ctx.shared.get::<CmcClient>();

        self.spawn_actor();
        self.spawn_refresh_task();

        Ok(())
    }

    /// Replies to `.r` with the evaluated calculation.
    #[command(
        ".r",
        "Evaluate a calculation with unit and currency conversions",
        event,
        spec = RINK
    )]
    async fn calculate(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let message = self.eval_and_format(command.args(), false).await?;

        client.send_privmsg(command.channel(), message)?;

        Ok(())
    }

    /// Replies to `.c` with the evaluated calculation, correcting unit casing.
    #[command(
        ".c",
        "Evaluate a calculation, correcting unit casing",
        event,
        spec = CLASSIFY
    )]
    async fn classify(
        &self,
        _ctx: &Context,
        client: &Client,
        command: &CommandEvent,
    ) -> Result<(), ZetaError> {
        let message = self.eval_and_format(command.args(), true).await?;

        client.send_privmsg(command.channel(), message)?;

        Ok(())
    }

    #[handler(shutdown)]
    async fn shutdown(&mut self, _ctx: &Context, _client: &Client) -> Result<(), ZetaError> {
        // The refresh task holds a handle, so it would keep the actor alive after the plugin
        // shuts down. Stopping it and dropping the last handle closes the mailbox, which ends
        // the actor's loop.
        if let Some(task) = self.refresh_task.take() {
            task.abort();
        }

        self.actor = None;

        Ok(())
    }
}

impl Rink {
    /// Moves the engine into the actor task that serializes evaluations.
    fn spawn_actor(&mut self) {
        let engine = self.engine.take().expect("the rink context is built once");

        self.actor = Some(Handle::spawn(engine));
    }

    /// Starts the background task that refreshes the currency data at the TTL.
    ///
    /// The task fetches on its first tick and again whenever the cached data expires, asking the
    /// actor to swap in a context rebuilt with the new rates. Requires the `currency` setting to
    /// have created an HTTP client and the actor to be running.
    fn spawn_refresh_task(&mut self) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(actor) = self.actor.clone() else {
            return;
        };

        let (currency, url) = (Arc::clone(&self.currency), self.currency_url.clone());
        let (cmc, crypto_enabled) = (self.cmc.clone(), self.crypto_enabled);

        self.refresh_task = Some(tokio::spawn(
            async move {
                debug!("starting currency refresh task");

                let mut interval = tokio::time::interval(currency::CURRENCY_CHECK_INTERVAL);
                interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

                loop {
                    interval.tick().await;

                    let refreshed = currency
                        .refresh(|| {
                            refresh_data(
                                &client,
                                &currency,
                                cmc.as_deref(),
                                crypto_enabled,
                                &url,
                                &actor,
                            )
                        })
                        .await;

                    if let Err(error) = refreshed {
                        warn!(%error, "could not refresh currency data");
                    }
                }
            }
            .instrument(tracing::info_span!("currency_refresh")),
        ));
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
    ///
    /// # Errors
    ///
    /// Returns a [`ZetaError`] when the rink actor has stopped.
    async fn eval_and_format(&self, line: &str, classify: bool) -> Result<String, ZetaError> {
        let actor = self
            .actor
            .as_ref()
            .ok_or_else(|| plugin_err(std::io::Error::other("the rink actor is not running")))?;

        let retry_line = match actor.eval(line.to_owned(), classify).await? {
            EvalOutcome::Answer(text) => return Ok(text),
            EvalOutcome::NeedsCurrency { error_reply, .. } if self.client.is_none() => {
                return Ok(error_reply);
            }
            EvalOutcome::NeedsCurrency { retry_line, .. } => retry_line,
        };

        // Currency data is missing: refresh what can still change the outcome, then
        // re-evaluate the same classified line, mirroring upstream rink's REPL, which fetches
        // on demand in the same situation.
        self.ensure_currency_cached().await;

        match actor.eval(retry_line, false).await? {
            EvalOutcome::Answer(text) => Ok(text),
            EvalOutcome::NeedsCurrency { error_reply, .. } => Ok(error_reply),
        }
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
        let Some(actor) = self.actor.as_ref() else {
            warn!("the rink actor is not running; skipping the currency refresh");

            return;
        };

        let refreshed = {
            let fetch = || {
                refresh_data(
                    client,
                    &self.currency,
                    self.cmc.as_deref(),
                    self.crypto_enabled,
                    &self.currency_url,
                    actor,
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
    fn crypto_table_pending(&self) -> bool {
        self.crypto_enabled
            && self.cmc.is_some()
            && self
                .currency
                .read(|data| data.is_none_or(|entry| entry.crypto.is_none()))
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
fn classify(ctx: &RinkContext, line: &str) -> String {
    let mut iter = TokenIterator::new(line.trim()).peekable();
    let mut query = parse_query(&mut iter);

    if fix_unit_casing(ctx, &mut query) {
        query.spans_to_string()
    } else {
        line.to_owned()
    }
}

/// Rewrites every `Expr::Unit` name in `query` that rink cannot resolve into its
/// case-corrected spelling, when exactly one registry name matches case-insensitively.
///
/// Names are drawn from the registry's defined units, substances, base units, their long forms
/// and pending dependencies — so lowercase ISO codes like `usd` correct to `USD` both while the
/// currency data is missing and once it is loaded. A candidate that equals the name itself is
/// not a rewrite: such a query is untouched. Returns whether anything was rewritten; an
/// untouched query must be evaluated verbatim, since re-rendering it could shift a parse that
/// only rink's own error handling can report correctly. A query that already failed to parse
/// is always left verbatim too — re-rendering it would flatten rink's structured error into
/// literal `<error: …>` text, which then reads as a missing unit.
fn fix_unit_casing(ctx: &RinkContext, query: &mut Query) -> bool {
    /// Returns the single registry name matching `name` case-insensitively.
    ///
    /// Candidates are the documented set: defined units, pending dependencies, substances and
    /// base units with their long forms. `None` means the name already matches (nothing to
    /// rewrite), nothing matched, or the match is ambiguous.
    fn resolve_case<'ctx>(ctx: &'ctx RinkContext, name: &str) -> Option<&'ctx str> {
        let mut candidates: Vec<&str> = ctx
            .registry
            .units
            .keys()
            .map(String::as_str)
            .chain(ctx.registry.missing_deps.keys().map(String::as_str))
            .chain(ctx.registry.substances.keys().map(String::as_str))
            .chain(ctx.registry.base_unit_long_names.keys().map(String::as_str))
            .chain(ctx.registry.base_units.iter().map(|unit| unit.id.as_str()))
            .filter(|candidate| candidate.eq_ignore_ascii_case(name))
            .collect();

        candidates.sort_unstable();
        candidates.dedup();

        match candidates.as_slice() {
            [only] if *only != name => Some(*only),
            _ => None,
        }
    }

    fn fix_expr(ctx: &RinkContext, expr: &mut Expr, fixed: &mut bool) {
        match expr {
            Expr::Unit { name } => {
                if ctx.lookup(name).is_none()
                    && let Some(resolved) = resolve_case(ctx, name)
                {
                    name.clear();
                    name.push_str(resolved);
                    *fixed = true;
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

    // A parse error makes the evaluation fail whatever the casing, and re-rendering would
    // turn rink's structured error into literal `<error: …>` text — leave the line alone so
    // rink reports the real parse error.
    if contains_parse_error(query) {
        return false;
    }

    let mut fixed = false;

    let (top, conversion) = match query {
        Query::Expr(expr) | Query::Factorize(expr) | Query::UnitsFor(expr) => (expr, None),
        Query::Convert(top, conversion, ..) => (top, Some(conversion)),
        Query::Search(_) | Query::Error(_) => return fixed,
    };

    fix_expr(ctx, top, &mut fixed);

    match conversion {
        Some(Conversion::Expr(bottom)) => fix_expr(ctx, bottom, &mut fixed),
        // A unit list (`1000 -> ft, in`) carries bare names instead of expressions, so it
        // needs its own pass: skipping it would correct the left-hand side only and report the
        // uncorrected list item as a missing unit.
        Some(Conversion::List(units)) => {
            for unit in units {
                if ctx.lookup(unit).is_none()
                    && let Some(resolved) = resolve_case(ctx, unit)
                {
                    unit.clear();
                    unit.push_str(resolved);
                    fixed = true;
                }
            }
        }
        _ => {}
    }

    fixed
}

/// Returns whether `query` carries a parse error: a top-level [`Query::Error`] or an
/// `Expr::Error` nested in an otherwise valid query.
///
/// Such a query fails evaluation however its units are cased, and re-rendering it would turn
/// rink's structured error message into literal text.
fn contains_parse_error(query: &Query) -> bool {
    fn expr_contains_error(expr: &Expr) -> bool {
        match expr {
            Expr::Error { .. } => true,
            Expr::BinOp(binop) => {
                expr_contains_error(&binop.left) || expr_contains_error(&binop.right)
            }
            Expr::UnaryOp(unaryop) => expr_contains_error(&unaryop.expr),
            Expr::Mul { exprs } => exprs.iter().any(expr_contains_error),
            Expr::Of { expr, .. } => expr_contains_error(expr),
            Expr::Call { args, .. } => args.iter().any(expr_contains_error),
            Expr::Quote { .. } | Expr::Const { .. } | Expr::Date { .. } | Expr::Unit { .. } => {
                false
            }
        }
    }

    match query {
        Query::Expr(expr) | Query::Factorize(expr) | Query::UnitsFor(expr) => {
            expr_contains_error(expr)
        }
        Query::Convert(top, conversion, ..) => {
            expr_contains_error(top)
                || matches!(conversion, Conversion::Expr(bottom) if expr_contains_error(bottom))
        }
        Query::Search(_) => false,
        Query::Error(_) => true,
    }
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
            Self::Text => COLOR,
            Self::Value => RESET,
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
    use zeta_http::HttpConfig;
    use zeta_test_support::{
        settings_tests,
        wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        },
    };
    use zeta_utils::irc::strip_formatting;

    /// Evaluates `line` through the actor, panicking if it has stopped.
    async fn evaluate(rink: &Rink, line: &str, classify: bool) -> String {
        rink.eval_and_format(line, classify)
            .await
            .expect("the rink actor must be running")
    }

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

    /// Builds a CoinMarketCap client configuration for the tests.
    fn test_client_config() -> coinmarketcap::Config {
        coinmarketcap::Config::default()
    }

    /// A ranked cryptocurrency listing fixture: Ethereum and a faked euro coin, plus a symbol
    /// rink's loader would reject.
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
    fn offline_plugin() -> Rink {
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
    async fn listings_server(status: u16) -> MockServer {
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
    async fn plugin_with_listings(status: u16) -> (Rink, MockServer, MockServer) {
        let fiat_server = currency_server(200).await;
        let listings_server = listings_server(status).await;

        let mut rink = test_plugin(Some(format!("{}/data/currency.json", fiat_server.uri())));
        rink.cmc = Some(Arc::new(
            CmcClient::with_base(
                "test-api-key",
                &test_client_config(),
                &listings_server.uri(),
            )
            .unwrap(),
        ));

        (rink, fiat_server, listings_server)
    }

    #[tokio::test]
    async fn numeric_results_render_the_value_in_the_default_color() {
        let rink = offline_plugin();

        assert_eq!(
            evaluate(&rink, "4 m", false).await,
            "\x0310> \x0f4 meter \x0310(\x0flength\x0310)"
        );
    }

    #[tokio::test]
    async fn errors_render_behind_an_error_label_with_the_input_reset() {
        let rink = offline_plugin();

        // A query without a close match carries no suggestion.
        let reply = evaluate(&rink, "wronginput", false).await;
        assert!(
            strip_formatting(&reply).contains("Error: No such unit wronginput"),
            "{reply}"
        );
        // The input is reset so an error cannot leak formatting into the rest of the line.
        assert!(reply.ends_with("\x0fwronginput"), "{reply}");
    }

    #[tokio::test]
    async fn ans_references_the_previous_result() {
        let rink = offline_plugin();

        evaluate(&rink, "4 m", false).await;

        let reply = strip_formatting(&evaluate(&rink, "ans * 2", false).await);
        assert!(reply.contains("8 meter"), "{reply}");
        assert!(reply.contains("length"), "{reply}");
    }

    #[tokio::test]
    async fn substances_render_every_property_and_label_as_a_value() {
        let rink = offline_plugin();

        let reply = strip_formatting(&evaluate(&rink, "egg", false).await);

        for expected in [
            "egg: USA large egg.",
            "mass_shelled = 50 gram (mass)",
            "mass_white = 30 gram (mass)",
            "mass_yolk = 18.6 gram (mass)",
            "volume = approx. 46824.75 millimeter^3 (volume)",
            "volume_white = approx. 29573.52 millimeter^3 (volume)",
            "volume_yolk = approx. 17251.22 millimeter^3 (volume)",
        ] {
            assert!(reply.contains(expected), "missing {expected:?}: {reply}");
        }
    }

    #[tokio::test]
    async fn a_unit_span_following_a_number_stays_inside_the_value_run() {
        let rink = offline_plugin();

        let reply = strip_formatting(&evaluate(&rink, "helium", false).await);
        assert!(
            reply.contains("atomic_number = 2 (dimensionless)"),
            "{reply}"
        );
        assert!(
            reply.contains("molar_mass = 4.002602 gram / mole (molar_mass)"),
            "{reply}"
        );
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
            let reply = strip_formatting(&evaluate(&rink, line, false).await);

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

        for (line, classify) in [("1 ETH to USD", false), ("1 eth to usd", true)] {
            let reply = strip_formatting(&evaluate(&rink, line, classify).await);
            assert!(reply.contains("2650.32 USD"), "{line}: {reply}");
            assert!(reply.contains("money"), "{line}: {reply}");
        }

        // 50_000 satoshis are 0.0005 bitcoin at the fixture's 65 000 USD price.
        let sats = strip_formatting(&evaluate(&rink, "50000 sats to usd", true).await);
        assert!(sats.contains("32.5 USD"), "{sats}");

        // A gwei stays convertible without being resolvable as fiat.
        let gwei = strip_formatting(&evaluate(&rink, "1 gwei to usd", true).await);
        assert!(gwei.contains("USD"), "{gwei}");
    }

    #[tokio::test]
    async fn synthesis_skips_colliding_and_invalid_symbols() {
        let (rink, _fiat, _listings) = plugin_with_listings(200).await;

        rink.ensure_currency_cached().await;

        // The fake EUR coin (priced 1.2) must not shadow the bundled euro: 1 EUR is 0.5 USD
        // under the fiat fixture, and the rejected `1INCH` symbol must not break the load.
        let reply = strip_formatting(&evaluate(&rink, "1 EUR to USD", false).await);
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

        let disabled = strip_formatting(&evaluate(&rink, "1 eth to usd", true).await);
        assert!(disabled.contains("Missing dependencies: ETH"), "{disabled}");

        // The fiat dataset is fine; the listings endpoint is not. The refresh fails as a
        // whole, so the last known context — without crypto — stays in place, and the
        // pending dependency still surfaces ETH.
        let (rink, _fiat, _listings) = plugin_with_listings(500).await;
        rink.ensure_currency_cached().await;

        let failed = strip_formatting(&evaluate(&rink, "1 eth to usd", true).await);
        assert!(failed.contains("Missing dependencies: ETH"), "{failed}");
    }

    #[tokio::test]
    async fn a_pending_crypto_table_is_retried_inline() {
        let fiat = currency_server(200).await;
        let listings = listings_server(500).await;

        let mut rink = test_plugin(Some(format!("{}/data/currency.json", fiat.uri())));
        rink.cmc = Some(Arc::new(
            CmcClient::with_base("test-api-key", &test_client_config(), &listings.uri()).unwrap(),
        ));

        rink.ensure_currency_cached().await;
        assert_eq!(listings.received_requests().await.unwrap().len(), 1);

        // The listings fetch failed, so the requested table is absent even though the fiat
        // entry is fresh: unlike an unresolvable dependency, this state a refetch can fix,
        // so the query retries the listings inline instead of waiting out the whole TTL.
        let reply = evaluate(&rink, "1 gwei to usd", true).await;
        assert!(reply.contains("Missing dependencies"), "{reply}");

        assert_eq!(listings.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn missing_fiat_answers_missing_dependencies() {
        let server = currency_server(500).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));

        let failed = strip_formatting(&evaluate(&rink, "1 USD", false).await);
        assert!(failed.contains("Missing dependencies: USD"), "{failed}");

        let rink = offline_plugin();

        let disabled = strip_formatting(&evaluate(&rink, "1 USD", false).await);
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
            let reply = evaluate(&rink, "1 gwei to usd", true).await;

            assert!(reply.contains("Missing dependencies"), "{reply}");
        }

        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn classify_case_corrects_unresolvable_units() {
        let server = currency_server(200).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));

        rink.ensure_currency_cached().await;

        let cases = [
            ("10 usd to dkk", "149.504 DKK"),
            ("10usd to dkk", "149.504 DKK"),
            ("10 USD to DKK", "149.504 DKK"),
            ("30 eur to jpy", "5115.6 JPY"),
        ];

        for (line, value) in cases {
            let reply = strip_formatting(&evaluate(&rink, line, true).await);
            assert!(reply.contains(value), "{line}: missing {value:?}: {reply}");
            assert!(reply.contains("money"), "{line}: {reply}");
        }
    }

    #[tokio::test]
    async fn classify_corrects_units_while_the_currency_data_is_missing() {
        let rink = offline_plugin();

        // The corrections land before any evaluation: the reply names the *tracked*
        // dependencies (USD), which only the rewritten `10 USD to DKK` can trigger.
        let reply = strip_formatting(&evaluate(&rink, "10 usd to dkk", true).await);
        assert!(reply.contains("Missing dependencies: USD"), "{reply}");
    }

    #[tokio::test]
    async fn classify_leaves_unclassifiable_tokens_alone() {
        let rink = offline_plugin();

        let cases = [
            ("50 gram", &["50 gram", "mass"][..]),
            ("banana", &["Error: No such unit banana"][..]),
            (
                "2 hours from now",
                &["No such unit from", "did you mean freon?"][..],
            ),
        ];

        for (line, expected) in cases {
            let reply = strip_formatting(&evaluate(&rink, line, true).await);

            for needle in expected {
                assert!(
                    reply.contains(needle),
                    "{line}: missing {needle:?}: {reply}"
                );
            }
        }
    }

    #[tokio::test]
    async fn classify_returns_unparseable_queries_unchanged() {
        let rink = offline_plugin();

        let reply = strip_formatting(&evaluate(&rink, "20:00 in tokyo", true).await);
        assert!(reply.contains("Error: Expected term, got `:`"), "{reply}");
    }

    #[tokio::test]
    async fn classify_reports_the_parser_error_of_a_malformed_query() {
        let rink = offline_plugin();

        // Re-rendering the parsed query would flatten the parser's error into a literal
        // `<error: …>` unit, so the line stays verbatim and rink reports the real problem.
        for line in ["egg $%^", "10 usd +", "5 usd -> dkk + "] {
            assert_eq!(
                evaluate(&rink, line, true).await,
                evaluate(&rink, line, false).await,
                "classification changed the outcome of {line:?}"
            );
        }
    }

    #[tokio::test]
    async fn classify_only_reports_rewrites_that_change_the_name() {
        let server = currency_server(200).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));
        rink.ensure_currency_cached().await;

        // A bare `Option::cloned` is ambiguous between the `&T` and `&mut T` impls.
        #[allow(clippy::redundant_closure_for_method_calls)]
        let data = rink
            .currency
            .read(|data| data.cloned())
            .expect("the fetched currency data must be cached");
        let ctx = build_context(Some(&data), None).expect("the fetched data must build a context");

        let rewritten = |line: &str| {
            let mut iter = TokenIterator::new(line.trim()).peekable();
            let mut query = parse_query(&mut iter);

            fix_unit_casing(&ctx, &mut query)
        };

        // A context lookup that misses a substance is not a rewrite: `egg` resolves through
        // the substance table during evaluation, so nothing about the name changes.
        assert!(!rewritten("egg"), "a no-op must not count as a rewrite");
        assert!(!rewritten("banana"), "no candidate means no rewrite");
        assert!(
            !rewritten("10 USD"),
            "an already correct name is not a rewrite"
        );
        assert!(rewritten("10usd"), "a genuine case mismatch is a rewrite");
    }

    #[tokio::test]
    async fn classify_corrects_the_units_of_a_unit_list() {
        let server = currency_server(200).await;
        let rink = test_plugin(Some(format!("{}/data/currency.json", server.uri())));
        rink.ensure_currency_cached().await;

        // Both list items are corrected, not just the left-hand side.
        assert_eq!(
            evaluate(&rink, "5 usd -> dkk, eur", true).await,
            evaluate(&rink, "5 USD -> DKK, EUR", false).await,
        );
    }

    #[tokio::test]
    async fn classify_evaluates_dates() {
        let rink = offline_plugin();

        // A fixed date keeps the assertions free of the clock and the host timezone.
        let reply = evaluate(&rink, "#2016-08-24#", true).await;
        // The datetime renders as a value, inside the run opened right after the marker.
        assert!(strip_formatting(&reply).contains("2016-08-24"), "{reply}");
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
            let strict = evaluate(&rink, line, false).await;
            let smart = evaluate(&rink, line, true).await;
            assert_eq!(strict, smart, "classification changed {line:?}");
        }
    }

    #[tokio::test]
    #[ignore = "needs network access"]
    async fn the_live_dataset_serves_fiat_and_bitcoin_queries() {
        let rink = test_plugin(None);

        rink.ensure_currency_cached().await;

        let fiat = evaluate(&rink, "100 usd to eur", true).await;
        assert!(fiat.contains("euro") && fiat.contains("money"), "{fiat}");

        let bitcoin = evaluate(&rink, "1 btc to usd", true).await;
        assert!(
            bitcoin.contains("USD") && bitcoin.contains("money"),
            "{bitcoin}"
        );
    }

    #[tokio::test]
    async fn the_plugin_registers_its_commands() {
        let mut subscriptions = Subscriptions::new();

        <Rink as Plugin>::new(
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
}
