//! Formatting of coin quotes as IRC messages.
//!
//! The quote is styled with the house colors: the scaffolding in cyan, the values — the symbol,
//! the price, and the change percentages — reset to the default color, with changes in green
//! when positive and red when negative.

use std::fmt::Write as _;

use coinmarketcap::model::QuoteData;
use zeta_plugin::irc::{COLOR, PLAIN_REPLY_PREFIX, RESET, unstyle_text};

/// Formats a coin quote as an IRC message, e.g.:
///
/// `\x0310> Bitcoin (\x0fBTC\x0310) is currently trading at\x0f $97231.50\x0310 (...)`
pub fn format_quote(quote: &QuoteData, currency: &str, sign: Option<&str>) -> String {
    let name = unstyle_text(&quote.name);
    let symbol = unstyle_text(&quote.symbol);

    // The reply is assembled in one buffer: the values are written as they are formatted
    // instead of into per-value strings that are then interpolated and copied again.
    let mut reply =
        String::with_capacity(PLAIN_REPLY_PREFIX.len() + name.len() + symbol.len() + 192);
    reply.push_str(PLAIN_REPLY_PREFIX);

    let Some(fiat) = quote.quote.get(currency) else {
        let _ = write!(
            reply,
            "{name} ({RESET}{symbol}{COLOR}) has no quote in {currency}"
        );

        return reply;
    };

    let _ = write!(
        reply,
        "{name} ({RESET}{symbol}{COLOR}) is currently trading at{RESET} "
    );

    match fiat.price {
        Some(price) => reply.push_str(&format_money(price, currency, sign)),
        None => reply.push_str("n/a"),
    }

    let _ = write!(reply, "{COLOR} (1 Hour Change:{RESET} ");
    push_change(&mut reply, fiat.percent_change_1h);
    let _ = write!(reply, "{COLOR} 24 Hour Change:{RESET} ");
    push_change(&mut reply, fiat.percent_change_24h);
    let _ = write!(reply, "{COLOR} 7 Day Change:{RESET} ");
    push_change(&mut reply, fiat.percent_change_7d);
    reply.push_str(COLOR);
    reply.push(')');

    reply
}

/// Appends a price change percentage in green (positive) or red (negative), followed by a reset.
fn push_change(reply: &mut String, change: Option<f64>) {
    let Some(change) = change else {
        let _ = write!(reply, "{RESET}n/a");

        return;
    };

    let code = if change.is_sign_negative() {
        "\x034"
    } else {
        "\x033"
    };
    let _ = write!(reply, "{code}{change:+.2}%{RESET}");
}

/// Formats a price with the currency sign when one is known, appending the currency code
/// otherwise.
fn format_money(price: f64, currency: &str, sign: Option<&str>) -> String {
    let price = format_price(price);

    sign.map_or_else(
        || format!("{price} {currency}"),
        |sign| format!("{sign}{price}"),
    )
}

/// Formats a price with two decimals when it is at least one unit, and with up to ten decimals
/// (trailing zeroes trimmed) otherwise, to preserve precision for micro-priced coins.
fn format_price(price: f64) -> String {
    if price.abs() >= 1.0 {
        format!("{price:.2}")
    } else {
        let formatted = format!("{price:.10}");

        formatted
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use coinmarketcap::model::FiatQuote;
    use zeta_utils::irc;

    use super::*;

    /// Builds a quote for `Bitcoin (BTC)` in USD.
    fn test_quote() -> QuoteData {
        QuoteData {
            name: "Bitcoin".to_string(),
            symbol: "BTC".to_string(),
            quote: HashMap::from([(
                "USD".to_string(),
                FiatQuote {
                    price: Some(97231.504),
                    percent_change_1h: Some(0.5),
                    percent_change_24h: Some(-1.2),
                    percent_change_7d: Some(10.25),
                },
            )]),
        }
    }

    #[test]
    fn formats_quote() {
        let formatted = format_quote(&test_quote(), "USD", Some("$"));

        assert_eq!(
            formatted,
            "\x0310> Bitcoin (\x0fBTC\x0310) is currently trading at\x0f $97231.50\x0310 \
         (1 Hour Change:\x0f \x033+0.50%\x0f\x0310 24 Hour Change:\x0f \
         \x034-1.20%\x0f\x0310 7 Day Change:\x0f \x033+10.25%\x0f\x0310)"
        );
    }

    #[test]
    fn strips_formatting_from_api_text() {
        let mut quote = test_quote();
        quote.name = "\x0310Fake\nName".to_string();
        quote.symbol = "B\x02TC".to_string();

        let formatted = irc::strip_formatting(&format_quote(&quote, "USD", Some("$")));

        assert!(formatted.contains("Fake Name (BTC)"), "{formatted}");
        assert!(!formatted.contains('\n'), "{formatted}");
    }

    #[test]
    fn formats_quote_without_a_sign_or_a_quote() {
        let mut quote = test_quote();
        quote.quote = HashMap::from([(
            "AUD".to_string(),
            FiatQuote {
                price: Some(1234.5),
                percent_change_1h: None,
                percent_change_24h: None,
                percent_change_7d: None,
            },
        )]);

        let unknown = irc::strip_formatting(&format_quote(&quote, "AUD", None));
        assert!(unknown.contains("1234.50 AUD"), "{unknown}");
        assert!(unknown.contains("n/a"), "{unknown}");

        let missing = irc::strip_formatting(&format_quote(&test_quote(), "DKK", None));
        assert!(missing.contains("has no quote in DKK"), "{missing}");
    }

    #[test]
    fn formats_prices() {
        assert_eq!(format_price(97231.504), "97231.50");
        assert_eq!(format_price(1.0), "1.00");
        assert_eq!(format_price(0.5), "0.5");
        assert_eq!(format_price(0.000_012_34), "0.00001234");
        assert_eq!(format_price(0.0), "0");
    }

    #[test]
    fn formats_money() {
        assert_eq!(format_money(97231.504, "USD", Some("$")), "$97231.50");
        assert_eq!(format_money(1234.5, "EUR", Some("€")), "€1234.50");
        assert_eq!(format_money(1234.5, "AUD", None), "1234.50 AUD");
    }

    #[test]
    fn formats_changes() {
        let cases = [
            (Some(0.5), "\x033+0.50%\x0f"),
            (Some(-1.2), "\x034-1.20%\x0f"),
            (Some(0.0), "\x033+0.00%\x0f"),
            (None, "\x0fn/a"),
        ];

        for (change, expected) in cases {
            let mut formatted = String::new();
            push_change(&mut formatted, change);

            assert_eq!(formatted, expected);
        }
    }
}
