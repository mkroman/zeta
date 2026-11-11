//! Helpers for formatting and parsing points in time and durations.
//!
//! Anchor-free durations are formatted through fixed-length unit tables ([`format_seconds_in_words`]),
//! while distances between two points in time are counted against the calendar
//! ([`DistanceInWordsExt`]), so leap years and variable month lengths are exact.

use std::fmt::{self, Write};
use std::time::Duration;

use crate::text::IteratorJoinExt;

#[cfg(feature = "chrono")]
use chrono::{DateTime, Datelike, Months, Utc};
#[cfg(feature = "time")]
use time::{Date, Duration as TimeDuration, Month, OffsetDateTime};

// The unit tables are part of the public API so callers (and benchmarks) do not have to
// redefine unit lengths: `UNITS_WITH_SECONDS` is the canonical fixed-length table for
// [`format_seconds_in_words`], `UNITS_TO_MINUTES` serves the `time` implementation, and the
// `SECONDS_PER_*` constants can be used to build custom tables. Calendar distances do not
// use fixed-length tables.

/// Seconds in a second.
pub const SECONDS_PER_SECOND: i64 = 1;
/// Seconds in a minute.
pub const SECONDS_PER_MINUTE: i64 = 60;
/// Seconds in an hour.
pub const SECONDS_PER_HOUR: i64 = 60 * SECONDS_PER_MINUTE;
/// Seconds in a day.
pub const SECONDS_PER_DAY: i64 = 24 * SECONDS_PER_HOUR;
/// Seconds in a week.
pub const SECONDS_PER_WEEK: i64 = 7 * SECONDS_PER_DAY;
/// Seconds in a (non-leap) year.
pub const SECONDS_PER_YEAR: i64 = 365 * SECONDS_PER_DAY;

/// Duration units in descending order, as `(seconds per unit, singular name)`.
const YEARS: (i64, &str) = (SECONDS_PER_YEAR, "year");
const WEEKS: (i64, &str) = (SECONDS_PER_WEEK, "week");
const DAYS: (i64, &str) = (SECONDS_PER_DAY, "day");
const HOURS: (i64, &str) = (SECONDS_PER_HOUR, "hour");
const MINUTES: (i64, &str) = (SECONDS_PER_MINUTE, "minute");
const SECONDS: (i64, &str) = (SECONDS_PER_SECOND, "second");

/// Duration units in descending order, down to and including seconds.
///
/// A convenience table for [`format_seconds_in_words`]; every year counts as 365 days. For
/// distances between two calendar dates use [`DistanceInWordsExt`].
pub const UNITS_WITH_SECONDS: &[(i64, &str)] = &[YEARS, WEEKS, DAYS, HOURS, MINUTES, SECONDS];

/// Duration units in descending order, down to and including minutes.
#[cfg(any(feature = "time", test))]
const UNITS_TO_MINUTES: &[(i64, &str)] = &[YEARS, WEEKS, DAYS, HOURS, MINUTES];

/// Duration units from hours down to minutes.
pub const HOURS_AND_MINUTES: &[(i64, &str)] = &[HOURS, MINUTES];

/// Formats a duration of `total_seconds` in words using the given fixed-length unit table,
/// e.g. `"1 year, 2 weeks, and 3 days"`.
///
/// The table treats every year as 365 days; for distances between two calendar dates, where
/// leap years and variable month lengths matter, use [`DistanceInWordsExt`] instead.
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
fn join_counts_in_words(parts: impl Iterator<Item = (i64, &'static str)>) -> String {
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

/// Formats points in time and durations in words, e.g. `"1 year, 2 weeks, and 3 days"`.
///
/// The implementations for points in time count whole calendar months, so leap years and
/// variable month lengths are exact. For a distance between two arbitrary points in time, use
/// [`DistanceInWordsExt`].
pub trait TimeInWordsExt {
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

#[cfg(feature = "chrono")]
impl TimeInWordsExt for DateTime<Utc> {
    fn in_words(&self) -> String {
        let now = Utc::now();

        if *self >= now {
            "0 minutes".to_string()
        } else {
            calendar_distance_in_words(*self, now)
        }
    }
}

#[cfg(feature = "time")]
impl TimeInWordsExt for TimeDuration {
    fn in_words(&self) -> String {
        format_seconds_in_words(self.whole_seconds(), UNITS_TO_MINUTES)
    }
}

#[cfg(feature = "time")]
impl TimeInWordsExt for OffsetDateTime {
    fn in_words(&self) -> String {
        let now = Self::now_utc();

        if *self >= now {
            "0 minutes".to_string()
        } else {
            calendar_distance_in_words(*self, now)
        }
    }
}

/// Formats calendar distances between points in time in words, e.g.
/// `"1 year, 2 months, and 3 days"`.
///
/// Unlike [`TimeInWordsExt`], which formats anchor-free durations with fixed-length units,
/// distances are counted against real calendar dates: leap years, month lengths, and
/// end-of-month clamping (January 31 plus one month is February 28 or 29) are all accounted
/// for. The units descend from years through months — then weeks below one month — days, hours,
/// minutes, and seconds.
///
/// Leap seconds are not counted separately: UTC civil time treats every day as 86,400 seconds,
/// and a parsed leap second is represented as its preceding second, so distances are unaffected
/// by leap-second insertions. Daylight saving time does not apply to UTC.
///
/// Implemented for `chrono::DateTime<Utc>` (feature `chrono`) and `time::OffsetDateTime`
/// (feature `time`).
pub trait DistanceInWordsExt {
    /// Formats the distance between `self` and `to` in words, e.g.
    /// `"1 year, 2 months, and 3 days"`.
    ///
    /// The distance is symmetric: either order of the two points in time produces the same
    /// text. Zero distances are formatted as `"0 minutes"`.
    #[must_use]
    fn distance_in_words(self, to: Self) -> String;

    /// Formats the distance from now until `self` in words, e.g. `"2 months and 3 days"`.
    ///
    /// Points in time in the past are formatted as `"0 minutes"`.
    #[must_use]
    fn time_until(self) -> String;
}

/// A point in time with just enough calendar arithmetic to measure distances in words.
#[cfg(any(feature = "chrono", feature = "time"))]
trait CalendarPoint: Copy + PartialOrd {
    /// Returns the year and the 1-based month of the point in time.
    fn year_and_month(self) -> (i64, u32);

    /// Adds `months` whole calendar months, clamping the day of month to the target month's
    /// length — e.g. January 31 plus one month is February 28 or 29.
    fn plus_months(self, months: u32) -> Option<Self>;

    /// Returns the whole seconds from `earlier` to `self`, which must not be earlier than
    /// `earlier`.
    ///
    /// The result is rounded to the nearest second, so points in time on exact boundaries are
    /// reported as such instead of a second short because of sub-second drift.
    fn seconds_since(self, earlier: Self) -> i64;
}

#[cfg(feature = "chrono")]
impl CalendarPoint for DateTime<Utc> {
    fn year_and_month(self) -> (i64, u32) {
        (i64::from(self.year()), self.month())
    }

    fn plus_months(self, months: u32) -> Option<Self> {
        self.checked_add_months(Months::new(months))
    }

    fn seconds_since(self, earlier: Self) -> i64 {
        let delta = self - earlier;
        let seconds = delta.num_seconds();

        if delta.subsec_nanos() >= 500_000_000 {
            seconds + 1
        } else {
            seconds
        }
    }
}

#[cfg(feature = "time")]
impl CalendarPoint for OffsetDateTime {
    fn year_and_month(self) -> (i64, u32) {
        (i64::from(self.year()), u8::from(self.month()).into())
    }

    fn plus_months(self, months: u32) -> Option<Self> {
        let index =
            i64::from(self.year()) * 12 + i64::from(u8::from(self.month()) - 1) + i64::from(months);
        let year = i32::try_from(index.div_euclid(12)).ok()?;
        let month = Month::try_from(u8::try_from(index.rem_euclid(12)).ok()? + 1).ok()?;
        let day = self.day().min(month.length(year));
        let date = Date::from_calendar_date(year, month, day).ok()?;

        Some(self.replace_date(date))
    }

    fn seconds_since(self, earlier: Self) -> i64 {
        let delta = self - earlier;
        let seconds = delta.whole_seconds();

        if delta.subsec_nanoseconds() >= 500_000_000 {
            seconds + 1
        } else {
            seconds
        }
    }
}

/// Decomposes the calendar distance from `from` to `to` into words, e.g.
/// `"1 year, 2 months, and 3 days"`.
#[cfg(any(feature = "chrono", feature = "time"))]
fn calendar_distance_in_words<C: CalendarPoint>(from: C, to: C) -> String {
    let (from, to) = if from <= to { (from, to) } else { (to, from) };

    let (from_year, from_month) = from.year_and_month();
    let (to_year, to_month) = to.year_and_month();
    let months_upper_bound =
        (to_year - from_year) * 12 + i64::from(to_month) - i64::from(from_month);

    // The month count is the upper bound or one less, because the day of month may not have
    // been reached yet — or, with end-of-month clamping, may have been reached early. Starting
    // one below and walking up therefore finds the true count in at most two steps.
    let mut months = u32::try_from(months_upper_bound.saturating_sub(1)).unwrap_or(0);

    while let Some(next) = from.plus_months(months + 1) {
        if next > to {
            break;
        }

        months += 1;
    }

    let anchor = from.plus_months(months).unwrap_or(from);
    let remainder = to.seconds_since(anchor);
    let days = remainder / SECONDS_PER_DAY;
    let hours = remainder % SECONDS_PER_DAY / SECONDS_PER_HOUR;
    let minutes = remainder % SECONDS_PER_HOUR / SECONDS_PER_MINUTE;
    let seconds = remainder % SECONDS_PER_MINUTE;

    join_counts_in_words(
        [
            (i64::from(months / 12), "year"),
            (i64::from(months % 12), "month"),
            (days / 7, "week"),
            (days % 7, "day"),
            (hours, "hour"),
            (minutes, "minute"),
            (seconds, "second"),
        ]
        .into_iter(),
    )
}

#[cfg(feature = "chrono")]
impl DistanceInWordsExt for DateTime<Utc> {
    fn distance_in_words(self, to: Self) -> String {
        calendar_distance_in_words(self, to)
    }

    fn time_until(self) -> String {
        let now = Utc::now();

        if self <= now {
            "0 minutes".to_string()
        } else {
            calendar_distance_in_words(now, self)
        }
    }
}

#[cfg(feature = "time")]
impl DistanceInWordsExt for OffsetDateTime {
    fn distance_in_words(self, to: Self) -> String {
        calendar_distance_in_words(self, to)
    }

    fn time_until(self) -> String {
        let now = Self::now_utc();

        if self <= now {
            "0 minutes".to_string()
        } else {
            calendar_distance_in_words(now, self)
        }
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
    let mut remainder = total_seconds;
    let mut formatted = String::with_capacity(32);
    let mut first = true;

    for &(unit_seconds, suffix) in UNITS {
        let count = remainder / unit_seconds;
        remainder %= unit_seconds;

        if count == 0 {
            continue;
        }

        if !first {
            formatted.push(' ');
        }

        let _ = write!(formatted, "{count}{suffix}");
        first = false;
    }

    if formatted.is_empty() {
        return "0s".to_string();
    }

    formatted
}

#[cfg(test)]
mod in_words_tests {
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

#[cfg(all(test, feature = "chrono"))]
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

    // The delta below sits inside a unit boundary with at least an hour of slack, so the drift
    // between the test's `Utc::now()` and the one inside `time_ago` cannot change the output.
    // The exact calendar boundary values are pinned clock-free by the `distance_in_words` tests.
    #[test]
    fn should_format_elapsed_time_in_words() {
        assert_eq!(
            (Utc::now() - (TimeDelta::weeks(2) - TimeDelta::hours(1))).time_ago(),
            "1 week, 6 days, and 23 hours ago"
        );
    }
}

#[cfg(all(test, feature = "chrono"))]
mod distance_in_words_tests {
    use chrono::{NaiveDate, TimeDelta, TimeZone};

    use super::*;

    fn utc(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).unwrap()
    }

    #[test]
    fn should_count_leap_day_anniversaries() {
        assert_eq!(
            utc(2024, 2, 28).distance_in_words(utc(2024, 2, 29)),
            "1 day"
        );
        assert_eq!(
            utc(2024, 2, 29).distance_in_words(utc(2025, 2, 28)),
            "1 year"
        );
        assert_eq!(
            utc(2024, 2, 29).distance_in_words(utc(2025, 3, 1)),
            "1 year and 1 day"
        );
    }

    #[test]
    fn should_apply_century_leap_year_rules() {
        // 2000 was a leap year, 2100 will not be.
        assert_eq!(
            utc(2000, 2, 28).distance_in_words(utc(2000, 3, 1)),
            "2 days"
        );
        assert_eq!(utc(2100, 2, 28).distance_in_words(utc(2100, 3, 1)), "1 day");
    }

    #[test]
    fn should_clamp_end_of_month_anniversaries() {
        assert_eq!(
            utc(2023, 1, 31).distance_in_words(utc(2023, 2, 28)),
            "1 month"
        );
        assert_eq!(
            utc(2024, 1, 31).distance_in_words(utc(2024, 2, 29)),
            "1 month"
        );
        assert_eq!(
            utc(2024, 1, 31).distance_in_words(utc(2024, 2, 28)),
            "4 weeks"
        );
    }

    #[test]
    fn should_format_weeks_only_below_one_month() {
        assert_eq!(
            utc(2024, 1, 1).distance_in_words(utc(2024, 1, 15)),
            "2 weeks"
        );
        assert_eq!(
            utc(2024, 1, 1).distance_in_words(utc(2024, 1, 20)),
            "2 weeks and 5 days"
        );
        assert_eq!(
            utc(2024, 1, 1).distance_in_words(utc(2024, 2, 16)),
            "1 month, 2 weeks, and 1 day"
        );
    }

    #[test]
    fn should_format_mixed_calendar_parts() {
        let from = Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0).unwrap();
        let to = Utc.with_ymd_and_hms(2025, 3, 2, 13, 30, 45).unwrap();

        assert_eq!(
            from.distance_in_words(to),
            "1 year, 2 months, 1 day, 1 hour, 30 minutes, and 45 seconds"
        );
    }

    #[test]
    fn should_format_distances_symmetrically() {
        let earlier = utc(1969, 7, 20);
        let later = utc(2024, 7, 20);

        assert_eq!(earlier.distance_in_words(later), "55 years");
        assert_eq!(later.distance_in_words(earlier), "55 years");
    }

    #[test]
    fn should_format_zero_distances_as_zero_minutes() {
        let at = utc(2024, 1, 1);

        assert_eq!(at.distance_in_words(at), "0 minutes");
    }

    #[test]
    fn should_count_distant_years() {
        assert_eq!(
            utc(2024, 1, 1).distance_in_words(utc(2400, 1, 1)),
            "376 years"
        );
    }

    #[test]
    fn should_fold_leap_seconds_into_the_previous_second() {
        let leap = NaiveDate::from_ymd_opt(2016, 12, 31)
            .unwrap()
            .and_hms_nano_opt(23, 59, 59, 1_000_000_000)
            .unwrap()
            .and_utc();
        let before = NaiveDate::from_ymd_opt(2016, 12, 31)
            .unwrap()
            .and_hms_opt(23, 59, 59)
            .unwrap()
            .and_utc();
        let midnight = utc(2017, 1, 1);

        assert_eq!(leap.distance_in_words(midnight), "0 minutes");
        assert_eq!(before.distance_in_words(midnight), "1 second");
    }

    #[test]
    fn should_format_time_until_from_now() {
        // A few days ahead leaves enough slack that the drift between the test's clock read
        // and the one inside `time_until` cannot change the leading unit.
        assert!(
            (Utc::now() + TimeDelta::days(2))
                .time_until()
                .starts_with("2 days")
        );
    }

    #[test]
    fn should_format_now_and_past_timestamps_as_zero_minutes_until() {
        assert_eq!(Utc::now().time_until(), "0 minutes");
        assert_eq!((Utc::now() - TimeDelta::days(1)).time_until(), "0 minutes");
    }
}

#[cfg(all(test, feature = "time"))]
mod offset_date_time_distance_tests {
    use time::Duration;

    use super::*;

    fn utc(year: i32, month: Month, day: u8) -> OffsetDateTime {
        Date::from_calendar_date(year, month, day)
            .unwrap()
            .with_hms(0, 0, 0)
            .unwrap()
            .assume_utc()
    }

    #[test]
    fn should_count_leap_day_anniversaries() {
        assert_eq!(
            utc(2024, Month::February, 28).distance_in_words(utc(2024, Month::February, 29)),
            "1 day"
        );
        assert_eq!(
            utc(2024, Month::February, 29).distance_in_words(utc(2025, Month::February, 28)),
            "1 year"
        );
        assert_eq!(
            utc(2024, Month::February, 29).distance_in_words(utc(2025, Month::March, 1)),
            "1 year and 1 day"
        );
    }

    #[test]
    fn should_apply_century_leap_year_rules() {
        // 2000 was a leap year, 2100 will not be.
        assert_eq!(
            utc(2000, Month::February, 28).distance_in_words(utc(2000, Month::March, 1)),
            "2 days"
        );
        assert_eq!(
            utc(2100, Month::February, 28).distance_in_words(utc(2100, Month::March, 1)),
            "1 day"
        );
    }

    #[test]
    fn should_clamp_end_of_month_anniversaries() {
        assert_eq!(
            utc(2023, Month::January, 31).distance_in_words(utc(2023, Month::February, 28)),
            "1 month"
        );
        assert_eq!(
            utc(2024, Month::January, 31).distance_in_words(utc(2024, Month::February, 29)),
            "1 month"
        );
        assert_eq!(
            utc(2024, Month::January, 31).distance_in_words(utc(2024, Month::February, 28)),
            "4 weeks"
        );
    }

    #[test]
    fn should_format_weeks_only_below_one_month() {
        assert_eq!(
            utc(2024, Month::January, 1).distance_in_words(utc(2024, Month::February, 16)),
            "1 month, 2 weeks, and 1 day"
        );
    }

    #[test]
    fn should_format_distances_symmetrically() {
        let earlier = utc(1969, Month::July, 20);
        let later = utc(2024, Month::July, 20);

        assert_eq!(earlier.distance_in_words(later), "55 years");
        assert_eq!(later.distance_in_words(earlier), "55 years");
    }

    #[test]
    fn should_format_elapsed_time_in_words() {
        let now = OffsetDateTime::now_utc();

        assert_eq!(now.in_words(), "0 minutes");
        assert_eq!((now + Duration::minutes(5)).in_words(), "0 minutes");

        let elapsed = (now - Duration::minutes(5)).time_ago();
        assert!(elapsed.starts_with("5 minutes"), "{elapsed}");
        assert!(elapsed.ends_with(" ago"), "{elapsed}");
    }

    #[test]
    fn should_format_time_until_from_now() {
        let now = OffsetDateTime::now_utc();

        assert!((now + Duration::days(2)).time_until().starts_with("2 days"));
        assert_eq!((now - Duration::days(1)).time_until(), "0 minutes");
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
