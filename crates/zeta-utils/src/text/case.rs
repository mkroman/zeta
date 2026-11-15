//! Case-insensitive prefix and suffix matching.

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
}
