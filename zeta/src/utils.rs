//! Shared utilities.
#![allow(unused)]

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
