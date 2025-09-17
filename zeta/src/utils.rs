use std::borrow::Cow;

/// Helpers for truncating text.
pub trait Truncatable {
    fn truncate_with_suffix(&self, len: usize, suffix: &str) -> Cow<'_, str>;
}

impl Truncatable for String {
    fn truncate_with_suffix(&self, len: usize, suffix: &str) -> Cow<'_, str> {
        self.as_str().truncate_with_suffix(len, suffix)
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
