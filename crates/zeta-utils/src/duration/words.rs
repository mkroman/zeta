//! Rendering fixed-length unit tables in words.

use std::fmt;

use crate::text::IteratorJoinExt;

/// Formats a duration of `total_seconds` in words using the given fixed-length unit table,
/// e.g. `"1 year, 2 weeks, and 3 days"`.
///
/// The table treats every year as 365 days; for distances between two calendar dates, where
/// leap years and variable month lengths matter, use
/// [`DistanceInWordsExt`](crate::duration::DistanceInWordsExt) instead.
///
/// Non-positive durations are formatted as `"0 minutes"`.
#[must_use]
pub fn format_seconds_in_words(total_seconds: i64, units: &[(i64, &'static str)]) -> String {
    join_counts_in_words(units.iter().scan(
        total_seconds.max(0),
        |remainder, &(unit_seconds, unit)| {
            let count = *remainder / unit_seconds;
            *remainder %= unit_seconds;
            Some((count, unit))
        },
    ))
}

/// One `count unit(s)` part of a duration, pluralized when formatted.
///
/// Rendering through [`fmt::Display`] keeps the parts streaming through [`IteratorJoinExt`]:
/// no per-part string is allocated.
struct WordCount {
    count: i64,
    unit: &'static str,
}

impl fmt::Display for WordCount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let plural = if self.count == 1 { "" } else { "s" };

        write!(formatter, "{} {}{plural}", self.count, self.unit)
    }
}

/// Joins `(count, unit)` parts as words, e.g. `"1 hour and 30 minutes"`, skipping zero counts.
///
/// Returns `"0 minutes"` when every count is zero.
pub(super) fn join_counts_in_words(parts: impl Iterator<Item = (i64, &'static str)>) -> String {
    let joined = parts
        .filter(|&(count, _)| count > 0)
        .map(|(count, unit)| WordCount { count, unit })
        .oxford_join();

    if joined.is_empty() {
        "0 minutes".to_string()
    } else {
        joined
    }
}

#[cfg(test)]
mod in_words_tests {
    use crate::duration::{
        HOURS_AND_MINUTES, SECONDS_PER_DAY, SECONDS_PER_HOUR, SECONDS_PER_MINUTE, SECONDS_PER_WEEK,
        SECONDS_PER_YEAR, UNITS_TO_MINUTES, UNITS_WITH_SECONDS,
    };

    use super::*;

    #[test]
    fn should_format_non_positive_durations_as_zero_minutes() {
        assert_eq!(format_seconds_in_words(0, UNITS_WITH_SECONDS), "0 minutes");
        assert_eq!(
            format_seconds_in_words(-42, UNITS_WITH_SECONDS),
            "0 minutes"
        );
    }

    #[test]
    fn should_format_single_units() {
        assert_eq!(
            format_seconds_in_words(SECONDS_PER_WEEK, UNITS_WITH_SECONDS),
            "1 week"
        );
        assert_eq!(
            format_seconds_in_words(30, UNITS_WITH_SECONDS),
            "30 seconds"
        );
        assert_eq!(
            format_seconds_in_words(2 * SECONDS_PER_WEEK, UNITS_WITH_SECONDS),
            "2 weeks"
        );
    }

    #[test]
    fn should_join_two_units_with_and() {
        assert_eq!(
            format_seconds_in_words(
                SECONDS_PER_HOUR + 30 * SECONDS_PER_MINUTE,
                UNITS_WITH_SECONDS
            ),
            "1 hour and 30 minutes"
        );
    }

    #[test]
    fn should_join_many_units_with_oxford_comma() {
        assert_eq!(
            format_seconds_in_words(
                SECONDS_PER_YEAR + SECONDS_PER_WEEK + SECONDS_PER_DAY,
                UNITS_WITH_SECONDS
            ),
            "1 year, 1 week, and 1 day"
        );
    }

    #[test]
    fn should_only_include_minutes_and_above_for_the_minutes_table() {
        assert_eq!(
            format_seconds_in_words(
                SECONDS_PER_HOUR + 30 * SECONDS_PER_MINUTE + 42,
                UNITS_TO_MINUTES
            ),
            "1 hour and 30 minutes"
        );
    }

    #[test]
    fn should_only_include_hours_and_minutes_for_the_hours_table() {
        assert_eq!(
            format_seconds_in_words(
                2 * SECONDS_PER_HOUR + 3 * SECONDS_PER_MINUTE,
                HOURS_AND_MINUTES
            ),
            "2 hours and 3 minutes"
        );
        assert_eq!(
            format_seconds_in_words(100 * SECONDS_PER_HOUR, HOURS_AND_MINUTES),
            "100 hours"
        );
    }
}
