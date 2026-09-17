#![allow(unused)]

use std::borrow::Cow;
use std::fmt::Write;

#[cfg(feature = "database")]
use chrono::{DateTime, Utc};
#[cfg(feature = "time")]
use time::Duration as TimeDuration;

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

/// Seconds in a minute.
const SECONDS_PER_MINUTE: i64 = 60;
/// Seconds in an hour.
const SECONDS_PER_HOUR: i64 = 60 * SECONDS_PER_MINUTE;
/// Seconds in a day.
const SECONDS_PER_DAY: i64 = 24 * SECONDS_PER_HOUR;
/// Seconds in a week.
const SECONDS_PER_WEEK: i64 = 7 * SECONDS_PER_DAY;
/// Seconds in a (non-leap) year.
const SECONDS_PER_YEAR: i64 = 365 * SECONDS_PER_DAY;
/// Seconds in a second.
const SECONDS_PER_SECOND: i64 = 1;

/// Duration units in descending order, as `(seconds per unit, singular name)`.
const YEARS: (i64, &str) = (SECONDS_PER_YEAR, "year");
const WEEKS: (i64, &str) = (SECONDS_PER_WEEK, "week");
const DAYS: (i64, &str) = (SECONDS_PER_DAY, "day");
const HOURS: (i64, &str) = (SECONDS_PER_HOUR, "hour");
const MINUTES: (i64, &str) = (SECONDS_PER_MINUTE, "minute");
const SECONDS: (i64, &str) = (SECONDS_PER_SECOND, "second");

/// Duration units in descending order, down to and including seconds.
const UNITS_WITH_SECONDS: &[(i64, &str)] = &[YEARS, WEEKS, DAYS, HOURS, MINUTES, SECONDS];

/// Duration units in descending order, down to and including minutes.
const UNITS_TO_MINUTES: &[(i64, &str)] = &[YEARS, WEEKS, DAYS, HOURS, MINUTES];

/// Formats points in time and durations in words, e.g. `"1 year, 2 weeks, and 3 days"`.
pub trait TimeInWords {
    /// Formats `self` in words, e.g. `"1 year, 2 weeks, and 3 days"`.
    ///
    /// Non-positive durations are formatted as `"0 minutes"`.
    #[must_use]
    fn in_words(&self) -> String;

    /// Formats the elapsed time since `self` in words, suffixed with `ago` — e.g.
    /// `"1 year and 5 weeks ago"`.
    #[must_use]
    fn time_ago(&self) -> String {
        let mut text = self.in_words();
        text.push_str(" ago");
        text
    }
}

#[cfg(feature = "database")]
impl TimeInWords for DateTime<Utc> {
    fn in_words(&self) -> String {
        words((Utc::now() - *self).num_seconds(), UNITS_WITH_SECONDS)
    }
}

#[cfg(feature = "time")]
impl TimeInWords for TimeDuration {
    fn in_words(&self) -> String {
        words(self.whole_seconds(), UNITS_TO_MINUTES)
    }
}

/// Formats a duration of `total_seconds` in words using the given unit table, e.g.
/// `"1 year, 2 weeks, and 3 days"`.
///
/// Non-positive durations are formatted as `"0 minutes"`.
fn words(total_seconds: i64, units: &[(i64, &'static str)]) -> String {
    if total_seconds <= 0 {
        return "0 minutes".to_string();
    }

    // Count the units that will appear in the output, so separators can be placed correctly.
    let mut num_parts = 0;
    let mut remainder = total_seconds;

    for &(unit_seconds, _) in units {
        if remainder / unit_seconds > 0 {
            num_parts += 1;
        }

        remainder %= unit_seconds;
    }

    if num_parts == 0 {
        return "0 minutes".to_string();
    }

    let last_separator = if num_parts > 2 { ", and " } else { " and " };
    // Sub-year remainders cap each non-leading count (weeks < 52, days < 7, ...) and large counts
    // only occur for years, so 24 bytes per part never needs a reallocation.
    let mut buf = String::with_capacity(24 * num_parts);
    let mut written_parts = 0;
    let mut remainder = total_seconds;

    for &(unit_seconds, name) in units {
        let count = remainder / unit_seconds;
        remainder %= unit_seconds;

        if count == 0 {
            continue;
        }

        written_parts += 1;

        if written_parts > 1 {
            buf.push_str(if written_parts == num_parts {
                last_separator
            } else {
                ", "
            });
        }

        let _ = write!(buf, "{count} {name}{}", if count == 1 { "" } else { "s" });
    }

    buf
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

#[cfg(test)]
mod words_tests {
    use super::*;

    #[test]
    fn should_format_non_positive_durations_as_zero_minutes() {
        assert_eq!(words(0, UNITS_WITH_SECONDS), "0 minutes");
        assert_eq!(words(-42, UNITS_WITH_SECONDS), "0 minutes");
    }

    #[test]
    fn should_format_single_units() {
        assert_eq!(words(SECONDS_PER_WEEK, UNITS_WITH_SECONDS), "1 week");
        assert_eq!(words(30, UNITS_WITH_SECONDS), "30 seconds");
        assert_eq!(words(2 * SECONDS_PER_WEEK, UNITS_WITH_SECONDS), "2 weeks");
    }

    #[test]
    fn should_join_two_units_with_and() {
        assert_eq!(
            words(
                SECONDS_PER_HOUR + 30 * SECONDS_PER_MINUTE,
                UNITS_WITH_SECONDS
            ),
            "1 hour and 30 minutes"
        );
    }

    #[test]
    fn should_join_many_units_with_oxford_comma() {
        assert_eq!(
            words(
                SECONDS_PER_YEAR + SECONDS_PER_WEEK + SECONDS_PER_DAY,
                UNITS_WITH_SECONDS
            ),
            "1 year, 1 week, and 1 day"
        );
    }

    #[test]
    fn should_only_include_minutes_and_above_for_the_minutes_table() {
        assert_eq!(
            words(SECONDS_PER_HOUR + 30 * SECONDS_PER_MINUTE + 42, UNITS_TO_MINUTES),
            "1 hour and 30 minutes"
        );
    }
}

#[cfg(all(test, feature = "time"))]
mod duration_in_words_tests {
    use time::Duration;

    use super::*;

    #[test]
    fn should_format_non_positive_durations_as_zero_minutes() {
        assert_eq!(Duration::ZERO.in_words(), "0 minutes");
        assert_eq!(Duration::minutes(-1).in_words(), "0 minutes");
    }

    #[test]
    fn should_format_durations_in_words() {
        assert_eq!(Duration::minutes(1).in_words(), "1 minute");
        assert_eq!(Duration::minutes(2).in_words(), "2 minutes");
        assert_eq!(Duration::weeks(2).in_words(), "2 weeks");
        assert_eq!(Duration::days(400).in_words(), "1 year and 5 weeks");
        assert_eq!(Duration::days(730).in_words(), "2 years");
    }

    #[test]
    fn should_suffix_time_ago() {
        assert_eq!(Duration::minutes(5).time_ago(), "5 minutes ago");
    }
}

#[cfg(all(test, feature = "database"))]
mod time_ago_tests {
    use chrono::TimeDelta;

    use super::*;

    #[test]
    fn should_format_now_and_future_timestamps_as_zero_minutes_ago() {
        assert_eq!(Utc::now().time_ago(), "0 minutes ago");
        assert_eq!(
            (Utc::now() + TimeDelta::hours(1)).time_ago(),
            "0 minutes ago"
        );
    }

    // The deltas below land exactly on a day boundary, so the drift between the test's
    // `Utc::now()` and the one inside `time_ago` cannot change the output unless the process
    // stalls for a full day. Smaller formatting cases are pinned by the clock-free `words` and
    // `time::Duration` tests instead.
    #[test]
    fn should_format_elapsed_time_in_words() {
        assert_eq!((Utc::now() - TimeDelta::weeks(2)).time_ago(), "2 weeks ago");
        assert_eq!(
            (Utc::now() - TimeDelta::days(400)).time_ago(),
            "1 year and 5 weeks ago"
        );
        assert_eq!((Utc::now() - TimeDelta::days(730)).time_ago(), "2 years ago");
    }
}
