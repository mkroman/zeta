//! Control-character removal for interpolated values.
//!
//! IRC formatting works through C0 control characters and DEL: bold (0x02),
//! color (0x03), hex color (0x04), reset (0x0f), monospace (0x11), reverse
//! (0x16), italic (0x1d), strikethrough (0x1e) and underline (0x1f). Since
//! every formatting character [modern IRC
//! formatting](https://modern.ircdocs.horse/formatting) defines is a control
//! character, removing control characters from user data removes every
//! formatting character the spec defines — user data cannot smuggle
//! formatting codes, carriage returns or line feeds into a reply.
//!
//! The filter is zero-cost for clean values (the overwhelmingly common
//! case): a character scan passes clean text through borrowed, and only
//! values that actually contain control characters pay for a filtered copy.

use std::borrow::Cow;

/// Returns `text` with all control characters removed.
///
/// Matches `char::is_control`: C0 (0x00–0x1f), DEL (0x7f) and C1 (0x80–0x9f)
/// are dropped. Formatting codes, carriage returns and line feeds in user
/// data never reach a reply.
#[must_use]
pub fn strip_control_chars(text: &str) -> Cow<'_, str> {
    let clean = text.chars().all(|c| !c.is_control());

    if clean {
        return Cow::Borrowed(text);
    }

    Cow::Owned(text.chars().filter(|c| !c.is_control()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every formatting character the IRC formatting spec defines.
    const FORMATTING_CODES: [char; 9] = [
        '\x02', '\x03', '\x04', '\x0f', '\x11', '\x16', '\x1d', '\x1e', '\x1f',
    ];

    #[test]
    fn clean_text_passes_through_borrowed() {
        let stripped = strip_control_chars("hello, world");

        assert!(matches!(stripped, Cow::Borrowed(_)));
        assert_eq!(stripped, "hello, world");
    }

    #[test]
    fn removes_every_formatting_code() {
        for code in FORMATTING_CODES {
            let text = format!("a{code}b");
            let stripped = strip_control_chars(&text);

            assert_eq!(stripped, "ab", "{code:?} must be removed");
        }
    }

    #[test]
    fn removes_carriage_returns_and_line_feeds() {
        assert_eq!(strip_control_chars("line\r\nbreak"), "linebreak");
        assert_eq!(strip_control_chars("line\nbreak"), "linebreak");
    }

    #[test]
    fn removes_del_and_c1_controls() {
        assert_eq!(strip_control_chars("a\u{7f}b"), "ab");
        assert_eq!(strip_control_chars("a\u{80}b"), "ab");
        assert_eq!(strip_control_chars("a\u{9f}b"), "ab");
    }

    #[test]
    fn keeps_printable_text() {
        assert_eq!(
            strip_control_chars("don't \u{201c}forget\u{201d} — naïve 🏳️‍🌈"),
            "don't \u{201c}forget\u{201d} — naïve 🏳️‍🌈"
        );
    }
}
