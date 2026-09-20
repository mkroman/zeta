//! Shared utilities.

use std::borrow::Cow;

/// Helpers for truncating text.
pub trait Truncatable {
    fn truncate_with_suffix(&self, len: usize, suffix: &str) -> Cow<'_, str>;

    /// Truncates the text so that it is at most `len` characters *including* `suffix`, which is
    /// appended when truncation happens.
    fn truncate_within(&self, len: usize, suffix: &str) -> String;
}

impl Truncatable for String {
    fn truncate_with_suffix(&self, len: usize, suffix: &str) -> Cow<'_, str> {
        self.as_str().truncate_with_suffix(len, suffix)
    }

    fn truncate_within(&self, len: usize, suffix: &str) -> String {
        self.as_str().truncate_within(len, suffix)
    }
}

impl Truncatable for str {
    fn truncate_with_suffix(&self, len: usize, suffix: &str) -> Cow<'_, str> {
        match self.char_indices().nth(len) {
            Some((byte_idx, _)) => {
                let mut truncated = String::with_capacity(byte_idx + suffix.len());
                truncated.push_str(&self[..byte_idx]);
                truncated.push_str(suffix);
                Cow::Owned(truncated)
            }
            None => Cow::Borrowed(self),
        }
    }

    fn truncate_within(&self, len: usize, suffix: &str) -> String {
        if self.chars().count() <= len {
            return self.to_string();
        }

        let mut truncated: String = self.chars().take(len.saturating_sub(1)).collect();
        truncated.push_str(suffix);
        truncated
    }
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

/// Collapses runs of whitespace in `value` into single spaces, trimming the result.
///
/// Text assembled from web pages carries line breaks and repeated spaces that an IRC message
/// cannot render: a line break terminates the message, and everything after it is lost.
#[must_use]
pub fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Removes every control character from `text`.
///
/// Control characters include all IRC formatting bytes, so text that is echoed back to a
/// channel must be stripped before it is formatted or replied with.
#[must_use]
pub fn strip_control_chars(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// Resolves a string setting: the configured value wins, then the `env` environment variable,
/// then the default.
#[must_use]
pub fn resolve_setting(setting: Option<&str>, env: &str, default: &str) -> String {
    setting
        .map(str::to_string)
        .or_else(|| std::env::var(env).ok())
        .unwrap_or_else(|| default.to_string())
}

/// Resolves an optional string setting: the configured value wins over the `env` environment
/// variable.
///
/// Empty values count as unset, so a blank configured value or environment variable does not
/// configure the setting.
#[must_use]
pub fn resolve_optional_setting(setting: Option<&str>, env: &str) -> Option<String> {
    setting
        .map(str::to_string)
        .or_else(|| std::env::var(env).ok())
        .filter(|value| !value.trim().is_empty())
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
/// append it themselves.
pub fn append_entries_within_budget(
    message: &mut String,
    entries: impl IntoIterator<Item = String>,
    separator: &str,
    budget: usize,
    reserved: usize,
) -> usize {
    let mut appended = 0usize;
    let mut first = true;

    for entry in entries {
        let prefix = if first { "" } else { separator };

        if !first && message.len() + prefix.len() + entry.len() + reserved > budget {
            break;
        }

        message.push_str(prefix);
        message.push_str(&entry);
        first = false;
        appended += 1;
    }

    appended
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_settings_in_priority_order() {
        assert_eq!(resolve_setting(Some("~meta"), "UNUSED_X_PREFIX", "reddit"), "~meta");
        assert_eq!(resolve_setting(None, "UNUSED_X_PREFIX", "reddit"), "reddit");
    }

    #[test]
    fn resolves_optional_settings_in_priority_order() {
        assert_eq!(
            resolve_optional_setting(Some("value"), "UNUSED_ENV_VAR"),
            Some("value".to_string())
        );
        assert_eq!(resolve_optional_setting(Some(""), "UNUSED_ENV"), None);
        assert_eq!(resolve_optional_setting(None, "UNUSED_ENV"), None);
        assert_eq!(resolve_optional_setting(Some("  "), "UNUSED_ENV"), None);
    }

    #[test]
    fn appends_entries_within_the_budget() {
        let mut message = String::from("header: ");

        let appended =
            append_entries_within_budget(&mut message, ["a", "b"].map(String::from), ", ", 30, 0);

        assert_eq!(appended, 2);
        assert_eq!(message, "header: a, b");
    }

    #[test]
    fn always_appends_the_first_entry_and_keeps_prefixes() {
        let mut message = String::from("header: ");

        let appended = append_entries_within_budget(
            &mut message,
            ["first", "second", "third"].map(String::from),
            ", ",
            20,
            0,
        );

        // The first entry is unconditional; "header: first, second" is 21 bytes and would
        // exceed the 20-byte budget, so the loop stops after the first entry.
        assert_eq!(appended, 1);
        assert_eq!(message, "header: first");
    }

    #[test]
    fn reserves_room_for_a_suffix() {
        let mut message = String::new();

        let appended = append_entries_within_budget(
            &mut message,
            ["first", "second"].map(String::from),
            ", ",
            10,
            8,
        );

        // "first" is unconditional; "second" (6 bytes) plus the separator and the 8 reserved
        // bytes exceeds the budget.
        assert_eq!(appended, 1);
        assert_eq!(message, "first");
    }

    #[test]
    fn truncate_string_with_suffix() {
        let string: String = "this is a very long string".to_string();

        assert_eq!(string.truncate_with_suffix(10, "…"), "this is a …");
        assert_eq!(
            string.truncate_with_suffix(250, "…"),
            "this is a very long string"
        );
    }

    #[test]
    fn truncate_within_budget_including_suffix() {
        let s: &str = "this is a very long string";

        assert_eq!(s.truncate_within(10, "…"), "this is a…");
        assert_eq!(s.truncate_within(30, "…"), "this is a very long string");
        assert_eq!("hæłlo".truncate_within(4, "…"), "hæł…");
    }

    #[test]
    fn truncate_str_with_suffix() {
        let s: &str = "this is a very long string";

        assert_eq!(s.truncate_with_suffix(10, "…"), "this is a …");
        assert_eq!(
            s.truncate_with_suffix(250, "…"),
            "this is a very long string"
        );
        // should not copy when length exceeds str
        assert!(matches!(s.truncate_with_suffix(250, "…"), Cow::Borrowed(_)));
        // should copy when truncating
        assert!(matches!(s.truncate_with_suffix(10, "…"), Cow::Owned(_)));
    }

    #[test]
    fn collapses_whitespace_runs_into_single_spaces() {
        // A line break terminates an IRC message, so text from web pages is collapsed.
        assert_eq!(collapse_whitespace("first\nsecond\nthird"), "first second third");
        assert_eq!(collapse_whitespace("  a   b\t\tc  "), "a b c");
        assert_eq!(collapse_whitespace("plain"), "plain");
        assert_eq!(collapse_whitespace("   "), "");
    }

    #[test]
    fn strips_control_characters() {
        // The IRC formatting bytes are control characters, so they must not survive into
        // user-controlled text that is echoed back to a channel.
        assert_eq!(strip_control_chars("a\u{2}b\u{f}c"), "abc");
        assert_eq!(strip_control_chars("line\nbreak"), "linebreak");
        assert_eq!(strip_control_chars("plain"), "plain");
    }
}
