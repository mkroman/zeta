//! Counting calendar distances between points in time.

#[cfg(feature = "chrono")]
use chrono::{DateTime, Datelike, Months, Utc};
#[cfg(feature = "time")]
use time::{Date, Month, OffsetDateTime};

#[cfg(any(feature = "chrono", feature = "time"))]
use super::words::join_counts_in_words;
#[cfg(any(feature = "chrono", feature = "time"))]
use super::{SECONDS_PER_DAY, SECONDS_PER_HOUR, SECONDS_PER_MINUTE};

/// Formats calendar distances between points in time in words, e.g.
/// `"1 year, 2 months, and 3 days"`.
///
/// Unlike [`TimeInWordsExt`](crate::duration::TimeInWordsExt), which formats anchor-free
/// durations with fixed-length units, distances are counted against real calendar dates: leap
/// years, month lengths, and end-of-month clamping (January 31 plus one month is February 28 or
/// 29) are all accounted for. The units descend from years through months — then weeks below one
/// month — days, hours, minutes, and seconds.
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
pub(super) trait CalendarPoint: Copy + PartialOrd {
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
pub(super) fn calendar_distance_in_words<C: CalendarPoint>(from: C, to: C) -> String {
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

    use crate::duration::TimeInWordsExt;

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
