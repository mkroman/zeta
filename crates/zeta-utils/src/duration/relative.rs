//! Formatting durations and points in time in words.

#[cfg(feature = "chrono")]
use chrono::{DateTime, Utc};
#[cfg(feature = "time")]
use time::{Duration as TimeDuration, OffsetDateTime};

#[cfg(any(feature = "chrono", feature = "time"))]
use super::distance::calendar_distance_in_words;
#[cfg(feature = "time")]
use super::{UNITS_TO_MINUTES, format_seconds_in_words};

/// Formats points in time and durations in words, e.g. `"1 year, 2 weeks, and 3 days"`.
///
/// The implementations for points in time count whole calendar months, so leap years and
/// variable month lengths are exact. For a distance between two arbitrary points in time, use
/// [`DistanceInWordsExt`](crate::duration::DistanceInWordsExt).
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
