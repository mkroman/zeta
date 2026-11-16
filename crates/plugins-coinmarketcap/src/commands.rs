//! The `.cc` and fixed coin command handlers, their command specs, and their reply mapping.
//!
//! Both command families quote a coin in a fiat currency; `.cc` resolves its coin argument by
//! symbol or fuzzy name first. Lookup failures are mapped to reply-grade sentences here, keeping
//! the client's operator-grade error detail out of the channel.

use tracing::warn;

use zeta_plugin::irc::{self, plain_reply, send_reply, usage_line};
use zeta_plugin::{Chain, Client, CommandSpec, ZetaError};
use zeta_utils::text::TruncatableExt;

use coinmarketcap::Error;
use coinmarketcap::model::CoinQuery;

use crate::CoinMarketCap;
use crate::args::{CoinOpts, QuoteOpts};
use crate::format;

/// The `.cc` command.
pub(super) const CC: CommandSpec =
    CommandSpec::with_args::<CoinOpts>(".cc", "Quote any coin by symbol or fuzzy name");

/// Defines the fixed coin commands and the [`COIN_COMMANDS`] table from one list of entries.
///
/// Keeping both in one place means a command cannot be registered without its symbol, or quoted
/// without being registered.
macro_rules! coin_commands {
    ($($spec:ident => $trigger:literal, $symbol:literal, $description:literal;)*) => {
        $(
            #[doc = concat!("The `", $trigger, "` command, quoting ", $symbol, ".")]
            pub(super) const $spec: CommandSpec =
                CommandSpec::with_args::<QuoteOpts>($trigger, $description);
        )*

        /// Fixed coin commands, mapped to the symbol they quote.
        pub(super) const COIN_COMMANDS: &[(CommandSpec, &str)] = &[$(($spec, $symbol),)*];
    };
}

coin_commands! {
    BTC => ".btc", "BTC", "Quote Bitcoin (BTC) in a fiat currency";
    ETH => ".eth", "ETH", "Quote Ethereum (ETH) in a fiat currency";
    ZCASH => ".zcash", "ZEC", "Quote Zcash (ZEC) in a fiat currency";
    ZEC => ".zec", "ZEC", "Quote Zcash (ZEC) in a fiat currency";
    ANS => ".ans", "NEO", "Quote Neo (NEO), formerly Antshares";
    NEO => ".neo", "NEO", "Quote Neo (NEO) in a fiat currency";
    STELLAR => ".stellar", "XLM", "Quote Stellar (XLM) in a fiat currency";
    XMR => ".xmr", "XMR", "Quote Monero (XMR) in a fiat currency";
    XRP => ".xrp", "XRP", "Quote XRP in a fiat currency";
    LTC => ".ltc", "LTC", "Quote Litecoin (LTC) in a fiat currency";
    ETC => ".etc", "ETC", "Quote Ethereum Classic (ETC) in a fiat currency";
    GOLEM => ".golem", "GNT", "Quote Golem (GNT) in a fiat currency";
    SIA => ".sia", "SC", "Quote Siacoin (SC) in a fiat currency";
    DOGE => ".doge", "DOGE", "Quote Dogecoin (DOGE) in a fiat currency";
    MAID => ".maid", "MAID", "Quote MaidSafeCoin (MAID) in a fiat currency";
    BCASH => ".bcash", "BCH", "Quote Bitcoin Cash (BCH) in a fiat currency";
    TRUMP => ".trump", "TRUMP", "Quote Trump (TRUMP) in a fiat currency";
}

/// The argument hint for the `.cc` command's usage line.
const CC_USAGE_HINT: &str = "<symbol> [currency]";

impl CoinMarketCap {
    /// Handles a `.cc` invocation, resolving `args` to a coin by symbol or fuzzy name.
    pub(super) async fn handle_cc(
        &self,
        client: &Client,
        channel: &str,
        args: &str,
    ) -> Result<(), ZetaError> {
        let opts = match CC.parse_shell::<CoinOpts>(args) {
            Ok(opts) if !opts.coin.trim().is_empty() => opts,
            _ => {
                send_reply(
                    client,
                    channel,
                    plain_reply(usage_line(CC.trigger(), CC_USAGE_HINT)),
                )?;
                return Ok(());
            }
        };

        self.ensure_coins_cached().await;

        // Prefer a cached coin (by symbol, then by fuzzy name); fall back to letting the API
        // resolve the query as a symbol (e.g. when the cache has not been populated yet, or the
        // coin is outside the cached top 1000).
        let query = match self.resolve_coin(&opts.coin) {
            Some(id) => CoinQuery::Id(id),
            None => CoinQuery::Symbol(opts.coin.to_ascii_uppercase()),
        };

        self.quote_and_reply(client, channel, query, opts.currency.as_deref())
            .await
    }

    /// Handles a fixed coin command invocation (e.g. `.btc`), quoting the given `symbol`.
    pub(super) async fn handle_coin(
        &self,
        command: CommandSpec,
        symbol: &str,
        client: &Client,
        channel: &str,
        args: &str,
    ) -> Result<(), ZetaError> {
        let Ok(opts) = command.parse_shell::<QuoteOpts>(args) else {
            send_reply(
                client,
                channel,
                plain_reply(usage_line(command.trigger(), "[currency]")),
            )?;
            return Ok(());
        };

        self.quote_and_reply(
            client,
            channel,
            CoinQuery::Symbol(symbol.to_string()),
            opts.currency.as_deref(),
        )
        .await
    }

    /// Validates and resolves the optional conversion currency, quotes `query` in it and
    /// replies to `channel` with the result.
    async fn quote_and_reply(
        &self,
        client: &Client,
        channel: &str,
        query: CoinQuery,
        currency: Option<&str>,
    ) -> Result<(), ZetaError> {
        self.ensure_fiat_cached().await;

        let currency = match parse_currency(currency, &self.default_currency, |symbol| {
            self.is_valid_currency(symbol)
        }) {
            Ok(currency) => currency,
            Err(err) => return Self::reply_error(client, channel, &err),
        };

        let quote = self.client.get_quote(query, &currency).await;
        let sign = self.fiat_sign(&currency);

        match quote {
            Ok(quote) => {
                send_reply(
                    client,
                    channel,
                    format::format_quote(&quote, &currency, sign.as_deref()),
                )?;
            }
            Err(err) => Self::reply_error(client, channel, &err)?,
        }

        Ok(())
    }

    /// Replies with the reply-grade message for a failed coin lookup, logging unexpected
    /// failures.
    fn reply_error(client: &Client, channel: &str, err: &Error) -> Result<(), ZetaError> {
        // Unknown coins and unsupported currencies are user errors: they are replied to
        // verbatim, without logging. Anything else is unexpected and worth a warning; the
        // operator-grade chain is logged because the client logs the failed request at debug.
        if !matches!(err, Error::NotFound | Error::InvalidCurrency(_)) {
            warn!(exception.message = %Chain(err), "coin lookup failed");
        }

        send_reply(client, channel, plain_reply(reply_message(err)))?;

        Ok(())
    }
}

/// The longest unsupported-currency value echoed in a reply, in characters.
pub(super) const MAX_CURRENCY_CHARS: usize = 16;

/// Formats the reply-grade sentence for a failed coin lookup.
///
/// The client [`Error`]'s `Display` is operator-facing: [`Error::Api`] carries the API's own
/// error message and [`Error::Deserialize`] the parse detail, neither of which a channel may
/// see. Only the unsupported currency — a value the user typed — is echoed back, styled out and
/// bounded so it cannot break or restyle the reply line.
pub(super) fn reply_message(err: &Error) -> String {
    let Error::InvalidCurrency(currency) = err else {
        return reply_context(err).to_owned();
    };

    format!(
        "unsupported currency: {}",
        irc::unstyle_text(currency).truncate_within(MAX_CURRENCY_CHARS, "…")
    )
}

/// The fixed reply sentence for a failed coin lookup that echoes no API-provided text.
const fn reply_context(err: &Error) -> &'static str {
    match err {
        Error::NotFound => "no such coin",
        Error::InvalidCurrency(_) => "unsupported currency",
        Error::Api(_) => "could not retrieve the coin information",
        Error::Build(_) | Error::Request(_) => "could not reach the CoinMarketCap API",
        Error::InvalidApiKey(_) => "the CoinMarketCap API key is invalid",
        Error::Deserialize(_) => "could not parse the response from CoinMarketCap",
        // `coinmarketcap::Error` is `#[non_exhaustive]` and lives in another crate, so an
        // exhaustive match is impossible here. A variant added there gets this fixed sentence
        // until it is given wording of its own; `reply_error` still logs it as unexpected.
        _ => "could not look up the coin",
    }
}

/// Parses and validates the optional conversion currency of a command invocation.
///
/// The input is sanitized before validation, so an unsupported currency can be echoed back to
/// the channel without leaking IRC formatting codes or other control characters. Validation is
/// delegated to `is_valid`, which accepts everything while the fiat cache has not been
/// populated, deferring validation to the API.
pub(super) fn parse_currency(
    currency: Option<&str>,
    default_currency: &str,
    is_valid: impl Fn(&str) -> bool,
) -> Result<String, Error> {
    let currency =
        irc::strip_control_chars(currency.unwrap_or(default_currency)).to_ascii_uppercase();

    if is_valid(&currency) {
        Ok(currency)
    } else {
        Err(Error::InvalidCurrency(currency))
    }
}
