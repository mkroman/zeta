//! Formatting counts and pluralizing nouns.

/// Returns `noun` with a trailing `s` appended unless `count` is one — e.g. `"view"` or
/// `"views"`. The count itself is formatted separately, so replies can group digits with
/// [`format_number`].
#[must_use]
pub fn pluralize_noun(count: u64, noun: &str) -> String {
    let suffix = if count == 1 { "" } else { "s" };

    format!("{noun}{suffix}")
}

/// Formats `count` with `noun`, pluralized by a trailing `s` unless `count` is one — e.g.
/// `"1 filter"` or `"3 filters"`.
#[must_use]
pub fn pluralize(count: usize, noun: &str) -> String {
    format!(
        "{count} {}",
        pluralize_noun(u64::try_from(count).unwrap_or(u64::MAX), noun)
    )
}

/// Formats `value` with thousands separators, using the English grouping convention.
///
/// Counts shown in replies — followers, views, votes — are routinely in the millions, where
/// `1,234,567` reads faster than the bare digits.
#[must_use]
pub fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let separators = (digits.len() - 1) / 3;
    let mut formatted = String::with_capacity(digits.len() + separators);

    for (index, digit) in digits.char_indices() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }

        formatted.push(digit);
    }

    formatted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_with_thousands_separators() {
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(999), "999");
        assert_eq!(format_number(1_000), "1,000");
        assert_eq!(format_number(1_234_567), "1,234,567");
        assert_eq!(format_number(u64::MAX), "18,446,744,073,709,551,615");
    }

    #[test]
    fn pluralizes_nouns_without_the_count() {
        assert_eq!(pluralize_noun(0, "view"), "views");
        assert_eq!(pluralize_noun(1, "view"), "view");
        assert_eq!(pluralize_noun(20_000, "view"), "views");
    }

    #[test]
    fn pluralizes_by_count() {
        assert_eq!(pluralize(0, "filter"), "0 filters");
        assert_eq!(pluralize(1, "filter"), "1 filter");
        assert_eq!(pluralize(2, "filter"), "2 filters");
    }
}
