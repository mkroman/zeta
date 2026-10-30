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
    ///
    /// A suffix longer than `len` is kept whole, so the result can exceed `len` in that case.
    fn truncate_within(&self, len: usize, suffix: &str) -> String {
        let text = self.as_ref();

        if text.chars().count() <= len {
            return text.to_string();
        }

        let text_len = len.saturating_sub(suffix.chars().count());
        let byte_len = text
            .char_indices()
            .nth(text_len)
            .map_or(text.len(), |(index, _)| index);
        let mut truncated = String::with_capacity(byte_len + suffix.len());
        truncated.push_str(&text[..byte_len]);
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

/// Splits a leading string literal quoted with `quote` from `input`.
///
/// Returns the literal including its surrounding quotes and the input following it. The literal
/// ends at the first unescaped `quote`, so backslash escapes such as `\"` are skipped.
///
/// Supported quotes are `"` and `'`, which share JavaScript's backslash escaping rules. Template
/// literals are not supported because `${…}` interpolation needs different parsing.
///
/// A double-quoted literal can be passed to `serde_json::from_str` when the escaped contents are
/// needed; this helper only locates the literal.
///
/// # Examples
///
/// ```
/// # use zeta_utils::text::quoted_string_literal;
/// assert_eq!(
///     quoted_string_literal(r#""nonce" rest"#, '"'),
///     Some((r#""nonce""#, " rest"))
/// );
/// assert_eq!(
///     quoted_string_literal(r"'nonce' rest", '\''),
///     Some((r"'nonce'", " rest"))
/// );
/// ```
#[must_use]
pub fn quoted_string_literal(input: &str, quote: char) -> Option<(&str, &str)> {
    if !matches!(quote, '"' | '\'') {
        return None;
    }

    let body = input.strip_prefix(quote)?;
    let mut escaped = false;

    for (index, character) in body.char_indices() {
        match (escaped, character) {
            (true, _) => escaped = false,
            (false, '\\') => escaped = true,
            (false, character) if character == quote => {
                let end = index + 2;
                return Some((&input[..end], &input[end..]));
            }
            _ => {}
        }
    }

    None
}

/// Joins `items` into a sentence: all but the last are separated by `", "`, and the last by
/// `" and "`, e.g. `"rock, pop and jazz"`.
///
/// An empty slice yields an empty string.
#[must_use]
pub fn join_with_and<S: AsRef<str>>(items: &[S]) -> String {
    let Some((last, rest)) = items.split_last() else {
        return String::new();
    };

    let mut joined = String::new();

    for (index, item) in rest.iter().enumerate() {
        if index > 0 {
            joined.push_str(", ");
        }

        joined.push_str(item.as_ref());
    }

    if !rest.is_empty() {
        joined.push_str(" and ");
    }

    joined.push_str(last.as_ref());
    joined
}

/// Formats `count` with `noun`, pluralized by a trailing `s` unless `count` is one — e.g.
/// `"1 filter"` or `"3 filters"`.
#[must_use]
pub fn pluralize(count: usize, noun: &str) -> String {
    let suffix = if count == 1 { "" } else { "s" };

    format!("{count} {noun}{suffix}")
}

/// Collapses runs of whitespace in `value` into single spaces, trimming the result.
///
/// Text assembled from web pages carries line breaks and repeated spaces that an IRC message
/// cannot render: a line break terminates the message, and everything after it is lost.
#[must_use]
pub fn collapse_whitespace(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    let mut first = true;

    for word in value.split_whitespace() {
        if !first {
            collapsed.push(' ');
        }

        collapsed.push_str(word);
        first = false;
    }

    collapsed
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
    fn splits_quoted_string_literals() {
        // The literal includes its quotes, and the remainder follows it.
        assert_eq!(
            quoted_string_literal(r#""abc" rest"#, '"'),
            Some((r#""abc""#, " rest"))
        );
        assert_eq!(quoted_string_literal(r#""""#, '"'), Some((r#""""#, "")));
        assert_eq!(
            quoted_string_literal(r"'abc' rest", '\''),
            Some((r"'abc'", " rest"))
        );

        // Escaped quotes do not terminate the literal.
        assert_eq!(
            quoted_string_literal(r#""a\"b" rest"#, '"'),
            Some((r#""a\"b""#, " rest"))
        );
        assert_eq!(
            quoted_string_literal(r"'a\'b' rest", '\''),
            Some((r"'a\'b'", " rest"))
        );

        // An escaped backslash does not escape the closing quote.
        assert_eq!(
            quoted_string_literal(r#""a\\" rest"#, '"'),
            Some((r#""a\\""#, " rest"))
        );

        assert_eq!(quoted_string_literal("abc", '"'), None);
        assert_eq!(quoted_string_literal("", '"'), None);
        assert_eq!(quoted_string_literal(r#""abc"#, '"'), None);
        assert_eq!(quoted_string_literal(r#""a\"#, '"'), None);

        // Template literals and other delimiters are intentionally unsupported.
        assert_eq!(quoted_string_literal("`abc` rest", '`'), None);
    }

    #[test]
    fn truncate_within_budget_including_suffix() {
        let s: &str = "this is a very long string";

        assert_eq!(s.truncate_within(10, "…"), "this is a…");
        assert_eq!(s.truncate_within(30, "…"), "this is a very long string");
        assert_eq!("hæłlo".truncate_within(4, "…"), "hæł…");

        // The suffix is budgeted by its character count, and an empty suffix truncates to
        // exactly the requested length.
        assert_eq!("hello".truncate_within(4, "..."), "h...");
        assert_eq!("hello".truncate_within(3, ""), "hel");
        assert_eq!("hello".truncate_within(5, ""), "hello");
    }

    #[test]
    fn truncate_within_keeps_the_exact_character_budget() {
        // Exactly at the budget, one over, and a suffix that fills the whole budget.
        assert_eq!("hello".truncate_within(5, "!"), "hello");
        assert_eq!("hello".truncate_within(6, "!"), "hello");
        assert_eq!("hello world".truncate_within(5, "!"), "hell!");
        assert_eq!("hello".truncate_within(1, "…"), "…");
        assert_eq!("hello".truncate_within(0, ""), "");
        assert_eq!("hello".truncate_within(0, "…"), "…");

        // Multibyte text is sliced on a character boundary, not a byte offset.
        assert_eq!("hæłlo".truncate_within(3, ""), "hæł");
        assert_eq!("hæłlo".truncate_within(2, "…"), "h…");
        assert_eq!("日本語です".truncate_within(4, "…"), "日本語…");
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
    fn joins_items_with_a_final_conjunction() {
        assert_eq!(join_with_and::<&str>(&[]), "");
        assert_eq!(join_with_and(&["a"]), "a");
        assert_eq!(join_with_and(&["a", "b"]), "a and b");
        assert_eq!(join_with_and(&["a", "b", "c"]), "a, b and c");
        assert_eq!(join_with_and(&["a".to_owned(), "b".to_owned()]), "a and b");
    }

    #[test]
    fn pluralizes_by_count() {
        assert_eq!(pluralize(0, "filter"), "0 filters");
        assert_eq!(pluralize(1, "filter"), "1 filter");
        assert_eq!(pluralize(2, "filter"), "2 filters");
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
