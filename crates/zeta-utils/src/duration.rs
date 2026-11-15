//! Duration formatting, time-in-words rendering and ISO 8601 parsing.
//!
//! Durations are rendered compactly ([`format_duration`]) or through the fixed-length unit
//! tables of [`format_seconds_in_words`]. [`TimeInWordsExt`] turns a duration or a point in time
//! into words (`"1 year, 2 weeks, and 3 days"`), and [`DistanceInWordsExt`] counts the distance
//! between two points in time against the calendar, so leap years and variable month lengths are
//! exact. [`parse_iso8601_duration`] parses ISO 8601 duration strings.
//!
//! The module is split by concern: `words` renders fixed-length unit tables, `relative` formats
//! durations and points in time in words, `distance` counts calendar distances, `parse` parses
//! ISO 8601 durations, and `format` renders durations compactly. The unit tables stay here, and
//! every public item is re-exported here, so `zeta_utils::duration::{…}` stays the import path
//! for all of them.
//!
//! The `chrono` and `time` features extend the traits to `chrono::DateTime<Utc>` and
//! `time::OffsetDateTime` respectively.

mod distance;
mod format;
mod parse;
mod relative;
mod words;

pub use distance::DistanceInWordsExt;
pub use format::format_duration;
pub use parse::parse_iso8601_duration;
pub use relative::TimeInWordsExt;
pub use words::format_seconds_in_words;

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
