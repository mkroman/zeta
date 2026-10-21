//! Rendering of rink's markup spans into styled one-line replies.

use rink_core::output::fmt::{FmtToken, Span, TokenFmt};
use rink_core::output::{QueryError, QueryReply};
use zeta_utils::utils::collapse_whitespace;

use crate::prelude::*;

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
            Self::Text => REPLY_COLOR,
            Self::Value => RESET,
        }
    }
}

/// Renders a successful evaluation's markup spans as a one-line IRC reply.
pub(super) fn render_reply(reply: &QueryReply) -> String {
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
pub(super) fn error_message(error: &QueryError) -> String {
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
