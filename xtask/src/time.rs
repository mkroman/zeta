//! UTC time and date helpers shared by metadata and waiver expiry.
//!
//! The xtask only needs whole seconds and days since the Unix epoch, so this avoids pulling
//! in a date-time crate.

use std::time::SystemTime;

/// Today, expressed as days since the Unix epoch (UTC).
#[must_use]
pub fn today() -> i64 {
    unix_now() / 86_400
}

/// Seconds since the Unix epoch, saturating rather than failing before 1970.
#[must_use]
pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

/// Parse a `YYYY-MM-DD` date into days since the Unix epoch.
pub fn parse_date(value: &str) -> Option<i64> {
    let mut parts = value.split('-');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

/// Howard Hinnant's `days_from_civil`, valid for the proleptic Gregorian calendar.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dates_as_epoch_days() {
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("2000-01-01"), Some(10_957));
        assert_eq!(parse_date("2024-02-29"), Some(19_782));
    }

    #[test]
    fn rejects_malformed_dates() {
        for value in [
            "",
            "2024",
            "2024-01",
            "2024-01-01-01",
            "2024-13-01",
            "2024-00-01",
            "2024-01-00",
            "2024-01-32",
            "not-a-date",
        ] {
            assert_eq!(parse_date(value), None, "{value:?} should be rejected");
        }
    }
}
