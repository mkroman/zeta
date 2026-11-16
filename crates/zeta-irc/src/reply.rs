//! Building the correctly formatted reply lines plugins send.
//!
//! A reply opens with a teal `>` marker (mIRC color 10) and may carry a bold label naming the
//! source; the message body follows in the reply color, with values emphasized by switching to
//! the default color. The constants and writers here spell out those exact byte sequences.

use std::fmt;
use std::fmt::Write as _;

/// The teal color (mIRC color 10) used for plugin replies.
pub const COLOR: &str = "\x0310";

/// Resets bold, color, and any other text formatting back to the default.
pub const RESET: &str = "\x0f";

/// Starts bold text.
pub const BOLD: &str = "\x02";

/// The teal `>` marker every reply opens with.
pub const REPLY_MARKER: &str = "\x0310>";

/// The complete prefix [`plain_reply`] writes before a message, including its separating space.
///
/// Callers that budget a reply in bytes can subtract this instead of re-deriving the format.
pub const PLAIN_REPLY_PREFIX: &str = "\x0310> ";

/// The reply body capacity reserved up front, in bytes.
///
/// IRC replies are capped at roughly 400 bytes and the common reply is well under 200, so
/// reserving a body makes the common reply allocate once instead of once per doubling of its
/// length.
const REPLY_BODY_RESERVE: usize = 256;

/// The bytes [`write_labeled_reply_prefix`] writes besides the label: the marker, the reset, the
/// two bold codes, the colour code, a space and the colon.
///
/// Callers that assemble a labeled reply in their own buffer can reserve this many bytes plus
/// the label's length before calling [`write_labeled_reply_prefix`].
pub const LABELED_PREFIX_BYTES: usize =
    REPLY_MARKER.len() + RESET.len() + BOLD.len() + 1 + 1 + BOLD.len() + COLOR.len() + 1;

/// Writes `message` into `reply`.
///
/// Outlined and non-generic: the reply builders are generic over the message's `Display` type,
/// and this body is identical for all of them.
#[inline(never)]
fn write_display(reply: &mut String, message: &dyn fmt::Display) {
    // Writing into a `String` cannot fail, but the `Display` implementation can; `format!`
    // panics in that case, so keep the same message instead of silently truncating the reply.
    write!(reply, "{message}").expect(
        "a formatting trait implementation returned an error when the underlying stream did not",
    );
}

/// Formats `message` as a reply labeled `label`.
///
/// The reply starts with [`REPLY_MARKER`], followed by the bold label, e.g.
/// `> Twitch: <message>`.
#[inline]
#[must_use]
pub fn labeled_reply(label: &str, message: impl fmt::Display) -> String {
    let mut reply = String::with_capacity(LABELED_PREFIX_BYTES + label.len() + REPLY_BODY_RESERVE);
    write_labeled_reply_prefix(&mut reply, label);
    write_display(&mut reply, &message);

    reply
}

/// Writes the bold-label prefix [`labeled_reply`] starts its replies with into `reply`.
///
/// Unlike [`labeled_reply`], the prefix can be written to an incrementally built message, e.g.
/// from a `Display` implementation or a caller that assembles the whole reply in one buffer.
pub fn write_labeled_reply_prefix(reply: &mut String, label: &str) {
    reply.push_str(REPLY_MARKER);
    reply.push_str(RESET);
    reply.push_str(BOLD);
    reply.push(' ');
    reply.push_str(label);
    reply.push(':');
    reply.push_str(BOLD);
    reply.push_str(COLOR);
    reply.push(' ');
}

/// Returns the bold-label prefix that [`labeled_reply`] starts its replies with.
///
/// Unlike [`labeled_reply`], the prefix can be written to an incrementally built message, e.g.
/// from a `Display` implementation.
#[must_use]
pub fn labeled_reply_prefix(label: &str) -> String {
    let mut prefix = String::with_capacity(LABELED_PREFIX_BYTES + label.len());
    write_labeled_reply_prefix(&mut prefix, label);

    prefix
}

/// Formats `message` as a reply without a label.
#[inline]
#[must_use]
pub fn plain_reply(message: impl fmt::Display) -> String {
    let mut reply = String::with_capacity(PLAIN_REPLY_PREFIX.len() + REPLY_BODY_RESERVE);
    reply.push_str(PLAIN_REPLY_PREFIX);
    write_display(&mut reply, &message);

    reply
}

/// The label a derived usage line opens with.
const USAGE_LABEL: &str = "Usage: ";

/// The fixed part of a derived usage line: the label and the space after the [`RESET`] that
/// separates the trigger from the argument hint.
const USAGE_LINE_BYTES: usize = USAGE_LABEL.len() + RESET.len() + 1;

/// Formats a command's usage line from its trigger and its argument hint.
///
/// [`plain_reply`] colors the whole reply, so the [`RESET`] after the trigger returns the
/// argument placeholders to the default color: `.dig <name>` becomes `Usage: .dig\x0f <name>`.
/// Commands that need a different line declare it whole instead; this covers the common case
/// where only the arguments differ.
///
/// Not `const`: the line is built in an owned [`String`].
#[inline]
#[must_use]
pub fn usage_line(trigger: &str, hint: &str) -> String {
    let mut line = String::with_capacity(USAGE_LINE_BYTES + trigger.len() + hint.len());
    line.push_str(USAGE_LABEL);
    line.push_str(trigger);
    line.push_str(RESET);
    line.push(' ');
    line.push_str(hint);

    line
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Display` implementation that always fails, like a broken external formatter.
    struct FailingDisplay;

    impl fmt::Display for FailingDisplay {
        fn fmt(&self, _formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            Err(fmt::Error)
        }
    }

    #[test]
    fn labeled_reply_wraps_the_message_in_the_label() {
        assert_eq!(
            labeled_reply("Twitch", "hello"),
            "\x0310>\x0f\x02 Twitch:\x02\x0310 hello"
        );
    }

    #[test]
    fn labeled_reply_is_its_prefix_followed_by_the_message() {
        assert_eq!(
            labeled_reply("Twitch", "hello"),
            format!("{}{}", labeled_reply_prefix("Twitch"), "hello")
        );
        assert_eq!(
            labeled_reply("", ""),
            format!("{}{}", labeled_reply_prefix(""), "")
        );
    }

    #[test]
    fn plain_reply_prefixes_the_message_with_the_marker() {
        assert_eq!(plain_reply("No results found"), "\x0310> No results found");
    }

    #[test]
    fn plain_reply_prefix_is_the_whole_marker() {
        assert_eq!(plain_reply(""), PLAIN_REPLY_PREFIX);
        assert_eq!(PLAIN_REPLY_PREFIX, "\x0310> ");
    }

    #[test]
    fn plain_reply_prefix_extends_the_reply_marker_by_one_space() {
        assert!(PLAIN_REPLY_PREFIX.starts_with(REPLY_MARKER));
        assert_eq!(PLAIN_REPLY_PREFIX.len(), REPLY_MARKER.len() + 1);
    }

    #[test]
    fn usage_line_resets_between_the_trigger_and_the_hint() {
        assert_eq!(usage_line(".dig", "<name>"), "Usage: .dig\x0f <name>");
        assert_eq!(usage_line(".help", ""), "Usage: .help\x0f ");
    }

    #[test]
    #[should_panic(
        expected = "a formatting trait implementation returned an error when the underlying stream did not"
    )]
    fn labeled_reply_panics_when_the_display_implementation_fails() {
        let _ = labeled_reply("Twitch", FailingDisplay);
    }

    #[test]
    #[should_panic(
        expected = "a formatting trait implementation returned an error when the underlying stream did not"
    )]
    fn plain_reply_panics_when_the_display_implementation_fails() {
        let _ = plain_reply(FailingDisplay);
    }
}
