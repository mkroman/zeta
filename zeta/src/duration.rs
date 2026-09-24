//! Helpers for formatting and parsing points in time and durations.

use std::fmt::Write;
use std::time::Duration;

#[cfg(feature = "database")]
use chrono::{DateTime, Utc};
#[cfg(feature = "time")]
use time::Duration as TimeDuration;

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
pub const UNITS_WITH_SECONDS: &[(i64, &str)] = &[YEARS, WEEKS, DAYS, HOURS, MINUTES, SECONDS];

/// Duration units in descending order, down to and including minutes.
pub const UNITS_TO_MINUTES: &[(i64, &str)] = &[YEARS, WEEKS, DAYS, HOURS, MINUTES];

/// Duration units from hours down to minutes.
pub const HOURS_AND_MINUTES: &[(i64, &str)] = &[HOURS, MINUTES];

/// Splits `total_seconds` into its non-zero `(count, unit)` parts, walking `units` from
/// largest to smallest and spending the remainder of each unit on the next smaller one.
fn split_into_units(total_seconds: i64, units: &[(i64, &'static str)]) -> Vec<(i64, &'static str)> {
    let mut parts = Vec::new();
    let mut remainder = total_seconds;

    for &(unit_seconds, unit) in units {
        let count = remainder / unit_seconds;
        remainder %= unit_seconds;

        if count > 0 {
            parts.push((count, unit));
        }
    }

    parts
}

/// Formats a duration of `total_seconds` in words using the given unit table, e.g.
/// `"1 year, 2 weeks, and 3 days"`.
///
/// Non-positive durations are formatted as `"0 minutes"`.
#[must_use]
pub fn words(total_seconds: i64, units: &[(i64, &'static str)]) -> String {
    // Sub-year remainders cap each non-leading count (weeks < 52, days < 7, ...) and large counts
    // only occur for years, so 24 bytes per part never needs a reallocation.
    let parts = split_into_units(total_seconds.max(0), units);

    if parts.is_empty() {
        return "0 minutes".to_string();
    }

    let num_parts = parts.len();
    let last_separator = if num_parts > 2 { ", and " } else { " and " };
    let mut buf = String::with_capacity(24 * num_parts);

    for (index, (count, unit)) in parts.iter().enumerate() {
        if index > 0 {
            buf.push_str(if index == num_parts - 1 {
                last_separator
            } else {
                ", "
            });
        }

        let _ = write!(buf, "{count} {unit}{}", if *count == 1 { "" } else { "s" });
    }

    buf
}

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

/// Parses an ISO 8601 duration string, e.g. `PT1H2M20S`, `P1DT2H20M5S`, `PT45S`, or `P1W`,
/// into a [`Duration`].
///
/// Any component may be fractional, e.g. `PT1.5H`, and components of zero may be omitted.
/// Months and years are not supported, as YouTube video durations never use them. Returns
/// [`None`] for invalid input.
#[must_use]
pub fn parse_iso8601_duration(input: &str) -> Option<Duration> {
    let mut rest = input.strip_prefix('P')?;
    if rest.is_empty() {
        return None;
    }

    let mut total = Duration::ZERO;
    let mut in_time_part = false;

    while !rest.is_empty() {
        if let Some(remainder) = rest.strip_prefix('T') {
            in_time_part = true;
            rest = remainder;
            if rest.is_empty() {
                return None;
            }

            continue;
        }

        let digits_end = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        let number = rest[..digits_end].parse::<f64>().ok()?;
        let mut designator_and_rest = rest[digits_end..].chars();
        let designator = designator_and_rest.next()?;
        rest = designator_and_rest.as_str();

        // The number of seconds represented by each designator.
        let seconds_per_unit = match (designator, in_time_part) {
            ('W', false) => 604_800.0,
            ('D', false) => 86_400.0,
            ('H', true) => 3_600.0,
            ('M', true) => 60.0,
            ('S', true) => 1.0,
            _ => return None,
        };

        let component = Duration::try_from_secs_f64(number * seconds_per_unit).ok()?;
        total = total.checked_add(component)?;
    }

    Some(total)
}

/// Formats a [`Duration`] compactly, e.g. `1h 2m 20s`, skipping components of zero.
#[must_use]
pub fn format_duration(duration: Duration) -> String {
    // Seconds per day, hour, minute, and second, in descending order.
    const UNITS: &[(i64, &str)] = &[
        (SECONDS_PER_DAY, "d"),
        (SECONDS_PER_HOUR, "h"),
        (SECONDS_PER_MINUTE, "m"),
        (SECONDS_PER_SECOND, "s"),
    ];

    // A duration of more than `i64::MAX` seconds would outlive the universe several times over;
    // saturate rather than wrap.
    let total_seconds = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);

    match split_into_units(total_seconds, UNITS).as_slice() {
        [] => "0s".to_string(),
        parts => parts
            .iter()
            .map(|(count, suffix)| format!("{count}{suffix}"))
            .collect::<Vec<_>>()
            .join(" "),
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
            words(
                SECONDS_PER_HOUR + 30 * SECONDS_PER_MINUTE + 42,
                UNITS_TO_MINUTES
            ),
            "1 hour and 30 minutes"
        );
    }

    #[test]
    fn should_only_include_hours_and_minutes_for_the_hours_table() {
        assert_eq!(
            words(
                2 * SECONDS_PER_HOUR + 3 * SECONDS_PER_MINUTE,
                HOURS_AND_MINUTES
            ),
            "2 hours and 3 minutes"
        );
        assert_eq!(
            words(100 * SECONDS_PER_HOUR, HOURS_AND_MINUTES),
            "100 hours"
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

    // The deltas below sit inside a unit boundary with at least an hour of slack, so the drift
    // between the test's `Utc::now()` and the one inside `time_ago` cannot change the output.
    // The exact boundary values are pinned clock-free by the `words` tests above, which the
    // `DateTime<Utc>` impl delegates to with `UNITS_WITH_SECONDS`.
    #[test]
    fn should_format_elapsed_time_in_words() {
        assert_eq!(
            (Utc::now() - (TimeDelta::weeks(2) - TimeDelta::hours(1))).time_ago(),
            "1 week, 6 days, and 23 hours ago"
        );
        assert_eq!(
            (Utc::now() - TimeDelta::days(401)).time_ago(),
            "1 year, 5 weeks, and 1 day ago"
        );
        assert_eq!(
            (Utc::now() - TimeDelta::days(731)).time_ago(),
            "2 years and 1 day ago"
        );
    }
}

#[cfg(test)]
mod iso8601_duration_tests {
    use super::*;

    #[test]
    fn parses_iso8601_durations() {
        assert_eq!(
            parse_iso8601_duration("PT1H2M20S"),
            Some(Duration::from_secs(3600 + 2 * 60 + 20)),
        );
        assert_eq!(
            parse_iso8601_duration("PT4M13S"),
            Some(Duration::from_secs(4 * 60 + 13)),
        );
        assert_eq!(
            parse_iso8601_duration("P1DT2H20M5S"),
            Some(Duration::from_secs(86_400 + 2 * 3_600 + 20 * 60 + 5)),
        );
        assert_eq!(
            parse_iso8601_duration("PT45S"),
            Some(Duration::from_secs(45))
        );
        assert_eq!(
            parse_iso8601_duration("P1W"),
            Some(Duration::from_hours(7 * 24)),
        );
        assert_eq!(parse_iso8601_duration("PT0S"), Some(Duration::ZERO));
        assert_eq!(
            parse_iso8601_duration("PT1.5S"),
            Some(Duration::from_millis(1500)),
        );
    }

    #[test]
    fn rejects_invalid_iso8601_durations() {
        assert_eq!(parse_iso8601_duration(""), None);
        assert_eq!(parse_iso8601_duration("P"), None);
        assert_eq!(parse_iso8601_duration("PT"), None);
        assert_eq!(parse_iso8601_duration("1H2M20S"), None);
        assert_eq!(parse_iso8601_duration("P1S"), None);
        assert_eq!(parse_iso8601_duration("P1M"), None);
        assert_eq!(parse_iso8601_duration("PT1X"), None);
        assert_eq!(parse_iso8601_duration("garbage"), None);
        assert_eq!(parse_iso8601_duration("P99999999999999999999999D"), None);
    }
}

#[cfg(test)]
mod format_duration_tests {
    use super::*;

    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;

    #[test]
    fn formats_durations_compactly() {
        assert_eq!(
            format_duration(Duration::from_secs(DAY + 2 * HOUR + 20 * MINUTE + 5)),
            "1d 2h 20m 5s",
        );
        assert_eq!(
            format_duration(Duration::from_secs(HOUR + 2 * MINUTE + 20)),
            "1h 2m 20s",
        );
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
        assert_eq!(format_duration(Duration::from_secs(DAY)), "1d");
        assert_eq!(format_duration(Duration::from_secs(DAY + HOUR)), "1d 1h");
        assert_eq!(format_duration(Duration::from_secs(MINUTE)), "1m");
        assert_eq!(format_duration(Duration::from_secs(MINUTE + 1)), "1m 1s");
        assert_eq!(format_duration(Duration::ZERO), "0s");
    }
}
