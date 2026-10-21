//! The coin commands and their argument handling.

use argh::{ArgsInfo, FromArgs};
use coinmarketcap::Error;
use zeta_utils::utils::strip_control_chars;

use crate::prelude::*;

/// The `.cc` command.
pub(super) const CC: CommandSpec =
    CommandSpec::with_args::<CoinOptions>(".cc", "Quote any coin by symbol or fuzzy name");

/// The `.btc` command.
const BTC: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".btc", "Quote Bitcoin (BTC) in a fiat currency");
/// The `.eth` command.
const ETH: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".eth", "Quote Ethereum (ETH) in a fiat currency");
/// The `.zcash` command.
const ZCASH: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".zcash", "Quote Zcash (ZEC) in a fiat currency");
/// The `.zec` command.
const ZEC: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".zec", "Quote Zcash (ZEC) in a fiat currency");
/// The `.ans` command (Antshares, the former name of Neo).
const ANS: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".ans", "Quote Neo (NEO), formerly Antshares");
/// The `.neo` command.
const NEO: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".neo", "Quote Neo (NEO) in a fiat currency");
/// The `.stellar` command.
const STELLAR: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".stellar", "Quote Stellar (XLM) in a fiat currency");
/// The `.xmr` command.
const XMR: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".xmr", "Quote Monero (XMR) in a fiat currency");
/// The `.xrp` command.
const XRP: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".xrp", "Quote XRP in a fiat currency");
/// The `.ltc` command.
const LTC: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".ltc", "Quote Litecoin (LTC) in a fiat currency");
/// The `.etc` command.
const ETC: CommandSpec = CommandSpec::with_args::<QuoteOptions>(
    ".etc",
    "Quote Ethereum Classic (ETC) in a fiat currency",
);
/// The `.golem` command.
const GOLEM: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".golem", "Quote Golem (GNT) in a fiat currency");
/// The `.sia` command.
const SIA: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".sia", "Quote Siacoin (SC) in a fiat currency");
/// The `.doge` command.
const DOGE: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".doge", "Quote Dogecoin (DOGE) in a fiat currency");
/// The `.maid` command.
const MAID: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".maid", "Quote MaidSafeCoin (MAID) in a fiat currency");
/// The `.bcash` command.
const BCASH: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".bcash", "Quote Bitcoin Cash (BCH) in a fiat currency");
/// The `.trump` command.
const TRUMP: CommandSpec =
    CommandSpec::with_args::<QuoteOptions>(".trump", "Quote Trump (TRUMP) in a fiat currency");

/// Fixed coin commands, mapped to the symbol they quote.
pub(super) const COIN_COMMANDS: &[(CommandSpec, &str)] = &[
    (BTC, "BTC"),
    (ETH, "ETH"),
    (ZCASH, "ZEC"),
    (ZEC, "ZEC"),
    (ANS, "NEO"),
    (NEO, "NEO"),
    (STELLAR, "XLM"),
    (XMR, "XMR"),
    (XRP, "XRP"),
    (LTC, "LTC"),
    (ETC, "ETC"),
    (GOLEM, "GNT"),
    (SIA, "SC"),
    (DOGE, "DOGE"),
    (MAID, "MAID"),
    (BCASH, "BCH"),
    (TRUMP, "TRUMP"),
];

/// The commands handled by this plugin.
pub(super) const COMMANDS: &[CommandSpec] = &[
    CC, BTC, ETH, ZCASH, ZEC, ANS, NEO, STELLAR, XMR, XRP, LTC, ETC, GOLEM, SIA, DOGE, MAID, BCASH,
    TRUMP,
];

/// Convert a coin price into another fiat currency.
#[derive(FromArgs, ArgsInfo, Debug)]
pub(super) struct QuoteOptions {
    /// fiat currency to convert the price into (defaults to USD)
    #[argh(positional)]
    pub(super) currency: Option<String>,
}

/// The usage hint for the `.cc` command.
pub(super) const CC_USAGE: &str = "Usage: .cc \x0f<symbol> [currency]";

/// Convert a cryptocurrency price into fiat currency.
#[derive(FromArgs, ArgsInfo, Debug)]
pub(super) struct CoinOptions {
    /// the coin to quote, by symbol or name
    #[argh(positional)]
    pub(super) coin: String,
    /// fiat currency to convert the price into (defaults to USD)
    #[argh(positional)]
    pub(super) currency: Option<String>,
}

/// Parses and validates the optional conversion currency of a command invocation.
///
/// The input is sanitized before validation, so an unsupported currency can be echoed back to
/// the channel without leaking IRC formatting control characters. Validation is delegated to
/// `is_valid`, which accepts everything while the fiat cache has not been populated,
/// deferring validation to the API.
pub(super) fn parse_currency(
    currency: Option<&str>,
    default_currency: &str,
    is_valid: impl Fn(&str) -> bool,
) -> Result<String, Error> {
    let currency = strip_control_chars(currency.unwrap_or(default_currency)).to_ascii_uppercase();

    if is_valid(&currency) {
        Ok(currency)
    } else {
        Err(Error::InvalidCurrency(currency))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_consistent() {
        assert!(COMMANDS.contains(&CC));

        for (spec, _) in COIN_COMMANDS {
            assert!(COMMANDS.contains(spec), "{spec:?} missing from COMMANDS");
        }

        let mut commands = COMMANDS
            .iter()
            .map(CommandSpec::trigger)
            .collect::<Vec<_>>();
        commands.sort_unstable();
        commands.dedup();
        assert_eq!(
            commands.len(),
            COMMANDS.len(),
            "duplicate command in COMMANDS"
        );
    }

    #[test]
    fn parses_command_options() {
        let options: QuoteOptions = BTC.parse_args("").unwrap();
        assert_eq!(options.currency, None);

        let options: QuoteOptions = BTC.parse_args("eur").unwrap();
        assert_eq!(options.currency.as_deref(), Some("eur"));

        let options: CoinOptions = CC.parse_args("bitcoin eur").unwrap();
        assert_eq!(options.coin, "bitcoin");
        assert_eq!(options.currency.as_deref(), Some("eur"));

        // `.cc` requires a coin.
        assert!(matches!(
            CC.parse_args::<CoinOptions>(""),
            Err(ArgumentError::Usage(_))
        ));
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
    }

    #[test]
    fn parses_currencies_without_fiat_cache() {
        // Without a fiat cache, validation is deferred to the API.
        assert_eq!(parse_currency(Some("xyz"), "USD", |_| true).unwrap(), "XYZ");
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
    }
}
