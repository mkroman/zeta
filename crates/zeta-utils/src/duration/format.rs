//! Rendering durations compactly.

use std::fmt::Write as _;
use std::time::Duration;

use super::{SECONDS_PER_DAY, SECONDS_PER_HOUR, SECONDS_PER_MINUTE, SECONDS_PER_SECOND};

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
