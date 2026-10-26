//! Rendering of rink's markup spans as one-line IRC replies.
//!
//! Rink's output is rendered from its markup spans rather than its plain text form, so the
//! styling follows the structure of the answer: the scaffolding stays cyan while the values —
//! numbers, datetimes, property names, echoed input in errors, category labels, and the unit
//! expressions that follow numbers — are reset to the default color. Control codes are emitted
//! only where the style actually changes, never duplicated, and never dangling at the end of
//! the message.

use rink_core::output::fmt::{FmtToken, Span, TokenFmt};
use rink_core::output::{QueryError, QueryReply};
use zeta_plugin::prelude::*;
use zeta_utils::text::collapse_whitespace;

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
pub fn render_reply(reply: &QueryReply) -> String {
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
pub fn error_message(error: &QueryError) -> String {
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
    use crate::test_support::{evaluate, offline_plugin};
    use zeta_utils::irc::strip_formatting;

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
    async fn classify_evaluates_dates() {
        let rink = offline_plugin();

        // A fixed date keeps the assertions free of the clock and the host timezone.
        let reply = evaluate(&rink, "#2016-08-24#", true).await;
        // The datetime renders as a value, inside the run opened right after the marker.
        assert!(strip_formatting(&reply).contains("2016-08-24"), "{reply}");
        // The value run closes right after the datetime's trailing timezone label.
        assert!(reply.contains("]\x0310"), "{reply}");
    }
}
