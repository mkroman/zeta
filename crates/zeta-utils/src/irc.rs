//! Helpers for IRC reply formatting, addressing, and line budgets.
//!
//! Plugin replies follow a common visual convention: a teal `>` marker (mIRC color 10), an
//! optional bold label naming the source, and the message body with values emphasized by
//! switching between the reply color and the default color. A reply with a label is a labeled
//! reply; one without is a plain reply. The helpers here produce those byte sequences so the
//! exact escape codes are written down in one place.

use std::fmt;
use std::iter::Peekable;
use std::str::Chars;

use crate::text::collapse_whitespace;

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

/// Formats `message` as a reply labeled `label`.
///
/// The reply starts with [`REPLY_MARKER`], followed by the bold label, e.g.
/// `> Twitch: <message>`.
#[must_use]
pub fn labeled_reply(label: &str, message: impl fmt::Display) -> String {
    format!("{}{}", labeled_reply_prefix(label), message)
}

/// Returns the bold-label prefix that [`labeled_reply`] starts its replies with.
///
/// Unlike [`labeled_reply`], the prefix can be written to an incrementally built message, e.g.
/// from a `Display` implementation.
#[must_use]
pub fn labeled_reply_prefix(label: &str) -> String {
    format!("{REPLY_MARKER}{RESET}{BOLD} {label}:{BOLD}{COLOR} ")
}

/// Formats `message` as a reply without a label.
#[must_use]
pub fn plain_reply(message: impl fmt::Display) -> String {
    format!("{PLAIN_REPLY_PREFIX}{message}")
}

/// Strips a nickname mention (`<nick>, ...` or `<nick>: ...`) from the start of `s`.
///
/// Plugins that react to being addressed use this to unwrap the mention and handle only the
/// text following it.
#[must_use]
pub fn strip_nick_prefix<'a>(s: &'a str, current_nickname: &'a str) -> Option<&'a str> {
    s.strip_prefix(current_nickname).and_then(|s| {
        if s.starts_with(", ") || s.starts_with(": ") {
            Some(&s[2..])
        } else {
            None
        }
    })
}

/// Removes every control character from `text`, including IRC formatting codes and their color
/// parameters.
///
/// Text that is echoed back to a channel must be stripped before it is formatted or replied
/// with: a line break would terminate the message, and formatting bytes would restyle it.
/// [`strip_formatting`] consumes the parameters that belong to a formatting code, so a color
/// code cannot leak its digits into the visible text. For text that may carry line breaks and
/// other whitespace, [`unstyle_text`] is the single-line variant.
#[must_use]
pub fn strip_control_chars(text: &str) -> String {
    strip_formatting_where(text, |character| !character.is_control())
}

/// Removes the IRC formatting characters from `text`, leaving the visible characters.
///
/// Implements every code in [the formatting specification](https://modern.ircdocs.horse/formatting):
/// bold, italics, underline, strikethrough, monospace, mIRC color, hex color, reverse color, and
/// reset. Color parameters follow the specification exactly: two digits are read when available,
/// and a trailing comma is only consumed when a background color follows it, so a comma meant as
/// text survives.
///
/// Unlike [`strip_control_chars`], this leaves non-formatting control characters such as line
/// breaks alone. Callers that echo untrusted text into a single IRC line want
/// [`unstyle_text`] instead.
#[must_use]
pub fn strip_formatting(text: &str) -> String {
    strip_formatting_where(text, |_| true)
}

/// Unstyles text for a single IRC line.
///
/// Formatting codes are removed, remaining control characters — including line breaks — become
/// spaces, and runs of whitespace are collapsed. This is the form callers want before echoing a
/// page title, an API description, or any other text they do not control: a line break would
/// terminate the IRC message, and formatting codes would restyle the rest of the reply.
#[must_use]
pub fn unstyle_text(text: &str) -> String {
    let text: String = strip_formatting(text)
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();

    collapse_whitespace(&text)
}

/// The shared implementation of [`strip_control_chars`] and [`strip_formatting`]: formatting
/// codes are consumed, and the remaining characters are kept only when `keep` returns `true`.
fn strip_formatting_where(text: &str, keep: impl Fn(char) -> bool) -> String {
    let mut visible = String::with_capacity(text.len());
    let mut characters = text.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '\u{3}' => strip_mirc_color(&mut characters),
            '\u{4}' => strip_hex_color(&mut characters),
            // The formatting toggles (bold, monospace, reverse, italics, strikethrough, and
            // underline) and the reset code.
            '\u{2}' | '\u{f}' | '\u{11}' | '\u{16}' | '\u{1d}' | '\u{1e}' | '\u{1f}' => {}
            character if keep(character) => visible.push(character),
            _ => {}
        }
    }

    visible
}

/// The number of hex digits in one hex-color component.
const HEX_COLOR_DIGITS: usize = 6;

/// Consumes an mIRC color's parameters, leaving a comma that introduces text.
fn strip_mirc_color(characters: &mut Peekable<Chars<'_>>) {
    if !characters.peek().is_some_and(char::is_ascii_digit) {
        return;
    }

    consume_color_digits(characters);

    let mut lookahead = characters.clone();
    if lookahead.next() == Some(',') && lookahead.peek().is_some_and(char::is_ascii_digit) {
        characters.next();
        consume_color_digits(characters);
    }
}

/// Consumes the one or two digits of an mIRC color component.
fn consume_color_digits(characters: &mut Peekable<Chars<'_>>) {
    for _ in 0..2 {
        if characters.peek().is_some_and(char::is_ascii_digit) {
            characters.next();
        } else {
            break;
        }
    }
}

/// Consumes a hex color's parameters, leaving a comma that introduces text.
fn strip_hex_color(characters: &mut Peekable<Chars<'_>>) {
    if !has_hex_color(characters) {
        return;
    }

    consume_hex_color(characters);

    let mut lookahead = characters.clone();
    if lookahead.peek() == Some(&',') {
        lookahead.next();
        if has_hex_color(&lookahead) {
            characters.next();
            consume_hex_color(characters);
        }
    }
}

/// Whether the next [`HEX_COLOR_DIGITS`] characters are hex digits.
fn has_hex_color(characters: &Peekable<Chars<'_>>) -> bool {
    let mut lookahead = characters.clone();

    (0..HEX_COLOR_DIGITS).all(|_| {
        let is_hex = lookahead.peek().is_some_and(char::is_ascii_hexdigit);
        lookahead.next();

        is_hex
    })
}

/// Consumes one hex-color component.
fn consume_hex_color(characters: &mut Peekable<Chars<'_>>) {
    for _ in 0..HEX_COLOR_DIGITS {
        characters.next();
    }
}

/// The maximum length of a listing, leaving room for the sender prefix, `PRIVMSG` framing and
/// line ending overhead within the classic 512-byte IRC line limit.
pub const MAX_LISTING_LENGTH: usize = 400;

/// Appends `entries` to `message`, separated by `separator`, as long as the listing —
/// including `reserved` trailing bytes — stays within `budget` bytes.
///
/// The first entry is always appended, so a count in the message header is never misleading;
/// subsequent entries are only appended while they fit. Once an entry does not fit, no further
/// entries are appended and earlier ones are kept.
///
/// Returns the number of entries appended.
///
/// Callers that trail a fixed `suffix` after the listing pass its length as `reserved` and
/// append it themselves. Entries may be owned or borrowed strings.
pub fn append_entries_within_budget<S: AsRef<str>>(
    message: &mut String,
    entries: impl IntoIterator<Item = S>,
    separator: &str,
    budget: usize,
    reserved: usize,
) -> usize {
    let mut appended = 0usize;
    let mut first = true;

    for entry in entries {
        let entry = entry.as_ref();
        let prefix = if first { "" } else { separator };

        if !first && message.len() + prefix.len() + entry.len() + reserved > budget {
            break;
        }

        message.push_str(prefix);
        message.push_str(entry);
        first = false;
        appended += 1;
    }

    appended
}

/// Appends `entries` to `message` like [`append_entries_within_budget`], adding the suffix
/// produced by `more` when not every entry fits.
///
/// `total` is the number of entries before budgeting, and `more` receives the number that were
/// left out. `reserved` covers suffixes the caller adds besides the truncation suffix, as with
/// [`append_entries_within_budget`]; the truncation suffix itself is measured here, so callers
/// cannot drift from the length `more` actually returns. Returns the number of entries
/// appended.
pub fn append_entries_with_more<S: AsRef<str>>(
    message: &mut String,
    entries: impl IntoIterator<Item = S>,
    total: usize,
    separator: &str,
    budget: usize,
    reserved: usize,
    more: impl Fn(usize) -> String,
) -> usize {
    // Reserve for the longest suffix, when every entry is dropped: over-reserving by a few
    // bytes for smaller counts is preferable to a suffix that pushes the line over budget.
    let suffix_reserve = more(total).len();

    let appended = append_entries_within_budget(
        message,
        entries,
        separator,
        budget,
        reserved + suffix_reserve,
    );
    let dropped = total.saturating_sub(appended);

    if dropped > 0 {
        message.push_str(&more(dropped));
    }

    appended
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labeled_reply_wraps_the_message_in_the_label() {
        assert_eq!(
            labeled_reply("Twitch", "hello"),
            "\x0310>\x0f\x02 Twitch:\x02\x0310 hello"
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
    fn appends_entries_within_the_budget() {
        let mut message = String::from("header: ");

        let appended = append_entries_within_budget(&mut message, ["a", "b"], ", ", 30, 0);

        assert_eq!(appended, 2);
        assert_eq!(message, "header: a, b");
    }

    #[test]
    fn always_appends_the_first_entry_and_keeps_prefixes() {
        let mut message = String::from("header: ");

        let appended =
            append_entries_within_budget(&mut message, ["first", "second", "third"], ", ", 20, 0);

        // The first entry is unconditional; "header: first, second" is 21 bytes and would
        // exceed the 20-byte budget, so the loop stops after the first entry.
        assert_eq!(appended, 1);
        assert_eq!(message, "header: first");
    }

    #[test]
    fn reserves_room_for_a_suffix() {
        let mut message = String::new();

        let appended = append_entries_within_budget(&mut message, ["first", "second"], ", ", 10, 8);

        // "first" is unconditional; "second" (6 bytes) plus the separator and the 8 reserved
        // bytes exceeds the budget.
        assert_eq!(appended, 1);
        assert_eq!(message, "first");
    }

    #[test]
    fn appends_a_suffix_for_the_entries_that_did_not_fit() {
        let mut message = String::new();

        let appended = append_entries_with_more(
            &mut message,
            ["first", "second", "third"],
            3,
            ", ",
            25,
            0,
            |dropped| format!(" (+{dropped} more)"),
        );

        assert_eq!(appended, 2);
        assert_eq!(message, "first, second (+1 more)");
    }

    #[test]
    fn the_truncation_suffix_counts_against_the_budget() {
        let mut message = String::new();

        let appended =
            append_entries_with_more(&mut message, ["first", "second"], 2, ", ", 20, 0, |_| {
                " (1 more)".to_string()
            });

        // "first, second" fits on its own, but the 9-byte suffix that would follow does not.
        assert_eq!(appended, 1);
        assert_eq!(message, "first (1 more)");
        assert!(message.len() <= 20);
    }

    #[test]
    fn does_not_append_the_suffix_when_every_entry_fits() {
        let mut message = String::new();

        // The suffix length is measured up front, even when every entry ends up fitting.
        let appended =
            append_entries_with_more(&mut message, ["first", "second"], 2, ", ", 40, 0, |_| {
                " (0 more)".to_string()
            });

        assert_eq!(appended, 2);
        assert_eq!(message, "first, second");
    }

    #[test]
    fn strips_control_characters() {
        // The IRC formatting bytes are control characters, so they must not survive into
        // user-controlled text that is echoed back to a channel.
        assert_eq!(strip_control_chars("a\u{2}b\u{f}c"), "abc");
        assert_eq!(strip_control_chars("line\nbreak"), "linebreak");
        assert_eq!(strip_control_chars("plain"), "plain");
        // The color parameters are not control characters themselves, but they belong to the
        // code and must not leak into the visible text.
        assert_eq!(strip_control_chars("\u{3}10USD"), "USD");
        assert_eq!(strip_control_chars("\u{4}FF0000red"), "red");
    }

    #[test]
    fn unstyles_text_for_a_single_reply_line() {
        // Formatting codes are removed, line breaks become spaces, and whitespace is collapsed.
        assert_eq!(unstyle_text("\x0310Fake\nTitle\x0f"), "Fake Title");
        assert_eq!(unstyle_text("first\nsecond"), "first second");
        assert_eq!(unstyle_text("  a \t b  "), "a b");
        assert_eq!(unstyle_text("plain"), "plain");
    }

    #[test]
    fn strips_every_formatting_toggle() {
        assert_eq!(
            strip_formatting(
                "\u{2}bold\u{2} \u{1d}italic\u{1d} \u{1f}underline\u{1f} \
                 \u{1e}strike\u{1e} \u{11}mono\u{11} \u{16}reverse\u{16} \u{f}reset"
            ),
            "bold italic underline strike mono reverse reset"
        );
    }

    #[test]
    fn strips_mirc_colors_and_consumes_their_parameters() {
        // Foreground only, foreground and background, and one/two digits.
        assert_eq!(strip_formatting("\u{3}10> hello"), "> hello");
        assert_eq!(strip_formatting("\u{3}01,02hello"), "hello");
        assert_eq!(strip_formatting("\u{3}1,2hello"), "hello");

        // A comma is text unless a background color follows it.
        assert_eq!(strip_formatting("\u{3},hello"), ",hello");
        assert_eq!(strip_formatting("\u{3}10,hello"), ",hello");
        assert_eq!(strip_formatting("\u{3},5hello"), ",5hello");
    }

    #[test]
    fn strips_hex_colors_and_consumes_their_parameters() {
        assert_eq!(strip_formatting("\u{4}FF0000red"), "red");
        assert_eq!(strip_formatting("\u{4}ff0000,00FF00green"), "green");

        // A comma is text unless a background color follows it, and fewer than six hex digits
        // are not a color code.
        assert_eq!(strip_formatting("\u{4}FF0000,green"), ",green");
        assert_eq!(strip_formatting("\u{4},red"), ",red");
        assert_eq!(strip_formatting("\u{4}FF00red"), "FF00red");
    }

    #[test]
    fn strip_formatting_matches_the_specification_example() {
        // https://modern.ircdocs.horse/formatting#examples
        assert_eq!(
            strip_formatting(
                "Rules: Don't spam 5\u{3}13,8,6\u{3},7,8, and especially not \u{2}9\u{2}\u{1d}!"
            ),
            "Rules: Don't spam 5,6,7,8, and especially not 9!"
        );
    }
}
