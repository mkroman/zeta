//! Text shaping and matching helpers.

use std::borrow::Cow;

/// Helpers for truncating text.
pub trait Truncatable: AsRef<str> {
    /// Truncates the text to at most `len` characters, appending `suffix` when truncation
    /// happens (the result may then exceed `len` by the suffix's length).
    fn truncate_with_suffix(&self, len: usize, suffix: &str) -> Cow<'_, str> {
        let text = self.as_ref();

        match text.char_indices().nth(len) {
            Some((byte_idx, _)) => {
                let mut truncated = String::with_capacity(byte_idx + suffix.len());
                truncated.push_str(&text[..byte_idx]);
                truncated.push_str(suffix);
                Cow::Owned(truncated)
            }
            None => Cow::Borrowed(text),
        }
    }

    /// Truncates the text so that it is at most `len` characters *including* `suffix`, which is
    /// appended when truncation happens.
    fn truncate_within(&self, len: usize, suffix: &str) -> String {
        let text = self.as_ref();

        if text.chars().count() <= len {
            return text.to_string();
        }

        let mut truncated: String = text.chars().take(len.saturating_sub(1)).collect();
        truncated.push_str(suffix);
        truncated
    }
}

impl<T: ?Sized + AsRef<str>> Truncatable for T {}

/// Strips `prefix` from `s`, comparing characters by their Unicode lowercase forms.
///
/// Unlike [`str::strip_prefix`], this matches case-insensitively, including non-ASCII letters
/// such as `Å`/`å`. The comparison applies simple character-wise case folding; full case folding
/// (`ß`/`SS`) is not implemented.
///
/// # Examples
///
/// ```
/// # use zeta_utils::text::strip_prefix_ignore_case;
/// assert_eq!(
///     strip_prefix_ignore_case("HVORnår åbner Netto?", "hvornår åbner "),
///     Some("Netto?")
/// );
/// ```
#[must_use]
pub fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let mut remaining = s;

    for expected in prefix.chars() {
        let actual = remaining.chars().next()?;
        if !chars_eq_ignore_case(actual, expected) {
            return None;
        }
        remaining = &remaining[actual.len_utf8()..];
    }

    Some(remaining)
}

/// Strips `suffix` from `s`, comparing characters by their Unicode lowercase forms.
///
/// Unlike [`str::strip_suffix`], this matches case-insensitively, including non-ASCII letters
/// such as `Å`/`å`. The comparison applies simple character-wise case folding; full case folding
/// (`ß`/`SS`) is not implemented.
///
/// # Examples
///
/// ```
/// # use zeta_utils::text::strip_suffix_ignore_case;
/// assert_eq!(
///     strip_suffix_ignore_case("Netto ÅBENT", " åbent"),
///     Some("Netto")
/// );
/// ```
#[must_use]
pub fn strip_suffix_ignore_case<'a>(s: &'a str, suffix: &str) -> Option<&'a str> {
    let mut characters = s.char_indices().rev();
    let mut start = s.len();

    for expected in suffix.chars().rev() {
        let (index, actual) = characters.next()?;
        if !chars_eq_ignore_case(actual, expected) {
            return None;
        }
        start = index;
    }

    Some(&s[..start])
}

/// Compares two characters by their Unicode lowercase forms.
fn chars_eq_ignore_case(left: char, right: char) -> bool {
    left.to_lowercase().eq(right.to_lowercase())
}

/// Collapses runs of whitespace in `value` into single spaces, trimming the result.
///
/// Text assembled from web pages carries line breaks and repeated spaces that an IRC message
/// cannot render: a line break terminates the message, and everything after it is lost.
#[must_use]
pub fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_prefixes_ignoring_case() {
        assert_eq!(
            strip_prefix_ignore_case("HVORnår åbner Café?", "hvornår åbner "),
            Some("Café?")
        );
        assert_eq!(strip_prefix_ignore_case("HELLO", "hello"), Some(""));
        assert_eq!(strip_prefix_ignore_case("hello", "world"), None);
        assert_eq!(strip_prefix_ignore_case("hell", "hello"), None);
        assert_eq!(strip_prefix_ignore_case("hello", ""), Some("hello"));
        assert_eq!(strip_prefix_ignore_case("", ""), Some(""));
        assert_eq!(strip_prefix_ignore_case("", "a"), None);
    }

    #[test]
    fn strips_suffixes_ignoring_case() {
        assert_eq!(
            strip_suffix_ignore_case("Café ÅBENT", " åbent"),
            Some("Café")
        );
        assert_eq!(strip_suffix_ignore_case("HELLO", "llo"), Some("HE"));
        assert_eq!(strip_suffix_ignore_case("hello", "world"), None);
        assert_eq!(strip_suffix_ignore_case("llo", "hello"), None);
        assert_eq!(strip_suffix_ignore_case("hello", ""), Some("hello"));
        assert_eq!(strip_suffix_ignore_case("", ""), Some(""));
        assert_eq!(strip_suffix_ignore_case("", "a"), None);
    }

    #[test]
    fn strip_helpers_preserve_character_boundaries() {
        assert_eq!(strip_prefix_ignore_case("æøåÆØÅ", "æøå"), Some("ÆØÅ"));
        assert_eq!(strip_suffix_ignore_case("æøåÆØÅ", "ÆØÅ"), Some("æøå"));
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
        assert_eq!(
            collapse_whitespace("first\nsecond\nthird"),
            "first second third"
        );
        assert_eq!(collapse_whitespace("  a   b\t\tc  "), "a b c");
        assert_eq!(collapse_whitespace("plain"), "plain");
        assert_eq!(collapse_whitespace("   "), "");
    }
}
