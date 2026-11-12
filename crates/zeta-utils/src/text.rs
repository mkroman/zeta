//! Text shaping, joining, and matching helpers.

use std::borrow::Cow;
use std::fmt::{self, Write as _};

mod article;

pub use article::indefinite_article;

/// Truncates `text` to at most `len` characters, appending `suffix` when truncation happens.
///
/// Non-generic so every receiver type shares one copy of the logic.
#[inline(never)]
fn truncate_with_suffix_str<'a>(text: &'a str, len: usize, suffix: &str) -> Cow<'a, str> {
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

/// Truncates `text` so that it is at most `len` characters *including* `suffix`.
///
/// Non-generic so every receiver type shares one copy of the logic.
#[inline(never)]
fn truncate_within_str<'a>(text: &'a str, len: usize, suffix: &str) -> Cow<'a, str> {
    // One character is at least one byte, so text within `len` bytes holds at most `len`
    // characters: the common short reply needs no scan at all.
    if text.len() <= len {
        return Cow::Borrowed(text);
    }

    // The text exceeds the budget in bytes, but the budget counts characters.
    if text.char_indices().nth(len).is_none() {
        return Cow::Borrowed(text);
    }

    truncate_with_suffix_str(text, len.saturating_sub(suffix.chars().count()), suffix)
}

/// Helpers for truncating text.
pub trait TruncatableExt: AsRef<str> {
    /// Truncates the text to at most `len` characters, appending `suffix` when truncation
    /// happens (the result may then exceed `len` by the suffix's length).
    fn truncate_with_suffix(&self, len: usize, suffix: &str) -> Cow<'_, str> {
        truncate_with_suffix_str(self.as_ref(), len, suffix)
    }

    /// Truncates the text so that it is at most `len` characters *including* `suffix`, which is
    /// appended when truncation happens.
    ///
    /// A suffix longer than `len` is kept whole, so the result can exceed `len` in that case.
    /// Text that fits is borrowed from `self`.
    fn truncate_within(&self, len: usize, suffix: &str) -> Cow<'_, str> {
        truncate_within_str(self.as_ref(), len, suffix)
    }
}

impl<T: ?Sized + AsRef<str>> TruncatableExt for T {}

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

/// Joining borrowed string lists into sentences.
///
/// Implemented for slices of string-like items — `&[&str]`, `Vec<String>`, and arrays (which
/// dereference to slices). The items are borrowed and the output is allocated once with its
/// exact capacity; for lazily produced values, use [`IteratorJoinExt`] instead.
pub trait ListJoinExt {
    /// Joins the items into a sentence with the Oxford comma: two items are joined by `and`,
    /// while the last of three or more follows a comma — `"rock and pop"` or
    /// `"rock, pop, and jazz"`.
    ///
    /// An empty list yields an empty string; a single item is returned unchanged.
    ///
    /// # Examples
    ///
    /// ```
    /// # use zeta_utils::text::ListJoinExt;
    /// assert_eq!(["rock", "pop", "jazz"].oxford_join(), "rock, pop, and jazz");
    /// assert_eq!(["rock", "pop"].oxford_join(), "rock and pop");
    /// ```
    #[must_use]
    fn oxford_join(self) -> String;

    /// Joins the items with a custom separator and final separator — e.g.
    /// `(", ", " and ")` renders the non-Oxford `"a, b and c"` style.
    ///
    /// `separator` goes between consecutive items, and `final_separator` before the last item
    /// of two or more. An empty list yields an empty string.
    ///
    /// # Examples
    ///
    /// ```
    /// # use zeta_utils::text::ListJoinExt;
    /// assert_eq!(
    ///     ["a", "b", "c"].join_with_separators(", ", " and "),
    ///     "a, b and c"
    /// );
    /// assert_eq!(["a", "b", "c"].join_with_separators(", ", ", "), "a, b, c");
    /// ```
    #[must_use]
    fn join_with_separators(self, separator: &str, final_separator: &str) -> String;
}

impl<S: AsRef<str>> ListJoinExt for &'_ [S] {
    fn oxford_join(self) -> String {
        join_strs(self, ", ", " and ", ", and ")
    }

    fn join_with_separators(self, separator: &str, final_separator: &str) -> String {
        join_strs(self, separator, final_separator, final_separator)
    }
}

/// Joining iterators of displayable items into sentences.
///
/// This is the [`ListJoinExt`] counterpart for values that do not exist as a slice, such as
/// `map`, `filter`, and `split` chains. The items are streamed into the output — at most one
/// item is held back to decide the final separator — so the iterator is consumed only once and
/// needs no intermediate [`Vec`].
pub trait IteratorJoinExt {
    /// Joins the items into a sentence with the Oxford comma; see
    /// [`ListJoinExt::oxford_join`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use zeta_utils::text::IteratorJoinExt;
    /// assert_eq!((1..=3).oxford_join(), "1, 2, and 3");
    /// assert_eq!("rock,pop,jazz".split(',').oxford_join(), "rock, pop, and jazz");
    /// ```
    #[must_use]
    fn oxford_join(self) -> String;

    /// Joins the items with a custom separator and final separator; see
    /// [`ListJoinExt::join_with_separators`].
    ///
    /// # Examples
    ///
    /// ```
    /// # use zeta_utils::text::IteratorJoinExt;
    /// assert_eq!(
    ///     (1..=3).join_with_separators(", ", ", and "),
    ///     "1, 2, and 3"
    /// );
    /// ```
    #[must_use]
    fn join_with_separators(self, separator: &str, final_separator: &str) -> String;
}

impl<I> IteratorJoinExt for I
where
    I: Iterator,
    I::Item: fmt::Display,
{
    fn oxford_join(self) -> String {
        join_display(self, ", ", " and ", ", and ")
    }

    fn join_with_separators(self, separator: &str, final_separator: &str) -> String {
        join_display(self, separator, final_separator, final_separator)
    }
}

/// Joins a slice of string-like items, borrowing each item and reserving the exact capacity.
fn join_strs<S: AsRef<str>>(
    items: &[S],
    separator: &str,
    conjunction: &str,
    long_conjunction: &str,
) -> String {
    let content_len: usize = items.iter().map(|item| item.as_ref().len()).sum();
    let separator_len = match items.len() {
        0 | 1 => 0,
        2 => conjunction.len(),
        count => (count - 2) * separator.len() + long_conjunction.len(),
    };

    let mut joined = String::with_capacity(content_len + separator_len);
    join_list_into(
        &mut joined,
        items.iter().map(AsRef::as_ref),
        separator,
        conjunction,
        long_conjunction,
    );
    joined
}

/// Joins arbitrary displayable items, reserving capacity from the iterator's lower size hint.
fn join_display<I, T>(
    items: I,
    separator: &str,
    conjunction: &str,
    long_conjunction: &str,
) -> String
where
    I: IntoIterator<Item = T>,
    T: fmt::Display,
{
    let items = items.into_iter();
    let mut joined = String::with_capacity(items.size_hint().0);
    join_list_into(&mut joined, items, separator, conjunction, long_conjunction);
    joined
}

/// TEMPORARY: deliberate overhead to verify that CI detects benchmark regressions.
///
/// This branch exists only to exercise the benchmark job; remove this probe before merging.
#[inline(never)]
fn benchmark_regression_probe() -> u64 {
    let mut value = std::hint::black_box(0_u64);
    let iterations = std::hint::black_box(64_u64);
    for index in 0..iterations {
        value = value.wrapping_add(index | 1);
    }
    value
}

/// Writes `items` into `out`, separating the parts as they are produced.
///
/// `conjunction` separates two items and `long_conjunction` the last of three or more — the
/// distinction is what makes the Oxford comma appear only where it belongs. One item is held
/// back at a time: an item is written only once the following one is known, which identifies
/// the final part without collecting the items first.
fn join_list_into<I, T>(
    out: &mut String,
    items: I,
    separator: &str,
    conjunction: &str,
    long_conjunction: &str,
) where
    I: IntoIterator<Item = T>,
    T: fmt::Display,
{
    std::hint::black_box(benchmark_regression_probe());
    let mut items = items.into_iter();
    let Some(mut pending) = items.next() else {
        return;
    };
    let mut written = 0;

    for item in items {
        if written > 0 {
            out.push_str(separator);
        }

        let _ = write!(out, "{pending}");
        written += 1;
        pending = item;
    }

    if written == 1 {
        out.push_str(conjunction);
    } else if written > 1 {
        out.push_str(long_conjunction);
    }

    let _ = write!(out, "{pending}");
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
    fn truncate_within_borrows_only_when_the_text_fits() {
        let s: &str = "this is a very long string";

        assert!(
            matches!(s.truncate_within(250, "…"), Cow::Borrowed(_)),
            "a budget larger than the text must stay borrowed so no allocation happens"
        );
        assert!(
            matches!(s.truncate_within(10, "…"), Cow::Owned(_)),
            "the suffix forces a new string, so the result must be owned"
        );
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
        assert!(
            matches!(s.truncate_with_suffix(250, "…"), Cow::Borrowed(_)),
            "a budget larger than the text must stay borrowed so no allocation happens"
        );
        assert!(
            matches!(s.truncate_with_suffix(10, "…"), Cow::Owned(_)),
            "the suffix forces a new string, so the result must be owned"
        );
    }

    #[test]
    fn joins_string_lists_with_an_oxford_comma() {
        let empty: &[&str] = &[];

        assert_eq!(empty.oxford_join(), "");
        assert_eq!(["a"].oxford_join(), "a");
        assert_eq!(["a", "b"].oxford_join(), "a and b");
        assert_eq!(["a", "b", "c"].oxford_join(), "a, b, and c");
        assert_eq!(["a", "b", "c", "d"].oxford_join(), "a, b, c, and d");
        assert_eq!(
            ["a".to_owned(), "b".to_owned(), "c".to_owned()].oxford_join(),
            "a, b, and c"
        );
        let unicode = ["æ".to_owned(), "ø".to_owned(), "å".to_owned()];
        assert_eq!(unicode.oxford_join(), "æ, ø, and å");
    }

    #[test]
    fn joins_display_iterators_with_an_oxford_comma() {
        assert_eq!((1..=3).oxford_join(), "1, 2, and 3");
        assert_eq!(std::iter::empty::<&str>().oxford_join(), "");
        assert_eq!("a,b,c".split(',').oxford_join(), "a, b, and c");
        assert_eq!(std::iter::once(1.5_f64).oxford_join(), "1.5");
    }

    #[test]
    fn joins_one_shot_iterators() {
        // `from_fn` is not `Clone`, so counting the parts upfront would not compile.
        let mut count = 0;
        let items = std::iter::from_fn(|| {
            count += 1;
            (count < 4).then_some(count)
        });

        assert_eq!(items.oxford_join(), "1, 2, and 3");
    }

    #[test]
    fn joins_with_custom_separators() {
        assert_eq!(
            ["a", "b", "c"].join_with_separators(", ", " and "),
            "a, b and c"
        );
        assert_eq!(["a", "b", "c"].join_with_separators(", ", ", "), "a, b, c");
        assert_eq!(["a", "b"].join_with_separators(", ", " or "), "a or b");
        assert_eq!(
            ["a", "b", "c"].join_with_separators("; ", "; and "),
            "a; b; and c"
        );
        assert_eq!(["a", "b", "c"].join_with_separators("", ""), "abc");
        assert_eq!(
            ["a", "b", "c"]
                .into_iter()
                .join_with_separators(", ", " and "),
            "a, b and c"
        );
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
