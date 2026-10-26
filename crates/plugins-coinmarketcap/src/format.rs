//! Formatting of coin quotes as IRC messages.
//!
//! The quote is styled with the house colors: the scaffolding in cyan, the values — the symbol,
//! the price, and the change percentages — reset to the default color, with changes in green
//! when positive and red when negative.

use coinmarketcap::model::QuoteData;
use zeta_plugin::prelude::*;

/// Formats a coin quote as an IRC message, e.g.:
///
/// `\x0310> Bitcoin (\x0fBTC\x0310) is currently trading at\x0f $97231.50\x0310 (...)`
pub fn format_quote(quote: &QuoteData, currency: &str, sign: Option<&str>) -> String {
    let name = &quote.name;
    let symbol = &quote.symbol;

    let Some(fiat) = quote.quote.get(currency) else {
        return notice(format!(
            "{name} ({RESET}{symbol}{COLOR}) has no quote in {currency}"
        ));
    };

    let price = fiat.price.map_or_else(
        || "n/a".to_string(),
        |price| format_money(price, currency, sign),
    );

    let change_hour = format_change(fiat.percent_change_1h);
    let change_day = format_change(fiat.percent_change_24h);
    let change_week = format_change(fiat.percent_change_7d);

    notice(format!(
        "{name} ({RESET}{symbol}{COLOR}) is currently trading at{RESET} {price}{COLOR} \
         (1 Hour Change:{RESET} {change_hour}{COLOR} 24 Hour Change:{RESET} {change_day}{COLOR} \
         7 Day Change:{RESET} {change_week}{COLOR})"
    ))
}

/// Formats a price change percentage in green (positive) or red (negative), followed by a reset.
fn format_change(change: Option<f64>) -> String {
    let Some(change) = change else {
        return "\x0fn/a".to_string();
    };

    if change.is_sign_negative() {
        format!("\x034{change:+.2}%\x0f")
    } else {
        format!("\x033{change:+.2}%\x0f")
    }
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
    use zeta_utils::irc::strip_formatting;

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

        let unknown = strip_formatting(&format_quote(&quote, "AUD", None));
        assert!(unknown.contains("1234.50 AUD"), "{unknown}");
        assert!(unknown.contains("n/a"), "{unknown}");

        let missing = strip_formatting(&format_quote(&test_quote(), "DKK", None));
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
        assert_eq!(format_change(Some(0.5)), "\x033+0.50%\x0f");
        assert_eq!(format_change(Some(-1.2)), "\x034-1.20%\x0f");
        assert_eq!(format_change(Some(0.0)), "\x033+0.00%\x0f");
        assert_eq!(format_change(None), "\x0fn/a");
    }
}
