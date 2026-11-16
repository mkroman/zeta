//! Truncating text to a character or a byte budget.

use std::borrow::Cow;

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

/// Truncates `text` to at most `len` bytes *including* `suffix`, cut on a character boundary.
///
/// Non-generic so every receiver type shares one copy of the logic.
#[inline(never)]
fn truncate_bytes_within_str<'a>(text: &'a str, len: usize, suffix: &str) -> Cow<'a, str> {
    // The common short reply needs no scan at all.
    if text.len() <= len {
        return Cow::Borrowed(text);
    }

    let cut = floor_char_boundary(text, len.saturating_sub(suffix.len()));
    let mut truncated = String::with_capacity(cut + suffix.len());
    truncated.push_str(&text[..cut]);
    truncated.push_str(suffix);

    Cow::Owned(truncated)
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

/// The largest index of `text` at or before `index` that is a character boundary.
const fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }

    index
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

    /// Truncates the text so that it is at most `len` bytes *including* `suffix`, cut on a
    /// character boundary.
    ///
    /// The byte-counted counterpart of [`TruncatableExt::truncate_within`], for callers whose
    /// budget is a wire limit in bytes (an IRC line, say) rather than a readable length. A
    /// suffix longer than `len` is kept whole, so the result can exceed `len` in that case.
    /// Text that fits is borrowed from `self`.
    fn truncate_bytes_within(&self, len: usize, suffix: &str) -> Cow<'_, str> {
        truncate_bytes_within_str(self.as_ref(), len, suffix)
    }
}

impl<T: ?Sized + AsRef<str>> TruncatableExt for T {}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn truncate_bytes_within_keeps_the_exact_byte_budget() {
        // Multibyte text is cut on a character boundary at or before the byte budget, suffix
        // included, so the result never exceeds the budget.
        assert_eq!("æææææ".truncate_bytes_within(8, "…"), "ææ…");
        assert_eq!("æææææ".truncate_bytes_within(9, "…"), "æææ…");
        assert_eq!("日本語です".truncate_bytes_within(7, "…"), "日…");
        assert_eq!("hæłlo".truncate_bytes_within(5, ""), "hæł");

        // Text that fits is borrowed and never cut.
        assert_eq!("hæłlo".truncate_bytes_within(7, "…"), "hæłlo");
        assert!(matches!(
            "hæłlo".truncate_bytes_within(7, "…"),
            Cow::Borrowed(_)
        ));

        // A suffix longer than the budget is kept whole.
        assert_eq!("ææææ".truncate_bytes_within(2, "…"), "…");
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
}
