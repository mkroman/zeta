//! The Google Places opening-hours model and its evaluation logic.

use serde::Deserialize;
use time::{
    Duration, OffsetDateTime, Time, format_description::FormatItem, macros::format_description,
};

/// Detailed information about a specific place.
#[derive(Debug, Deserialize)]
pub struct PlaceDetails {
    pub(super) name: String,
    opening_hours: Option<OpeningHours>,
    /// The offset from UTC in minutes.
    utc_offset: Option<i32>,
}

/// Container for opening hours information.
#[derive(Debug, Deserialize)]
struct OpeningHours {
    open_now: Option<bool>,
    periods: Option<Vec<Period>>,
}

/// Represents a single opening period (e.g., Monday 9:00 - 17:00).
#[derive(Debug, Deserialize, Clone)]
struct Period {
    open: TimePoint,
    close: Option<TimePoint>,
}

/// A specific point in time consisting of a day of the week and a time string.
#[derive(Debug, Deserialize, Clone)]
struct TimePoint {
    /// 0 = Sunday, 1 = Monday, ..., 6 = Saturday.
    day: u8,
    /// Time in 24-hour "HHMM" format.
    time: String,
}

impl PlaceDetails {
    /// Returns the current time at the place, taking its UTC offset into account.
    /// Defaults to UTC if no offset is provided.
    pub(super) fn local_now(&self) -> OffsetDateTime {
        let now = OffsetDateTime::now_utc();

        self.utc_offset.map_or(now, |offset_minutes| {
            now + Duration::minutes(i64::from(offset_minutes))
        })
    }

    /// Checks if the place is currently open based on the `open_now` field.
    pub(super) fn is_open_now(&self) -> bool {
        self.opening_hours
            .as_ref()
            .and_then(|oh| oh.open_now)
            .unwrap_or(false)
    }

    /// Checks if the place is open 24/7.
    ///
    /// This is determined by a specific pattern in the API response:
    /// a single period starting at day 0, time "0000" with no close time.
    pub(super) fn is_always_open(&self) -> bool {
        if let Some(oh) = &self.opening_hours
            && let Some(periods) = &oh.periods
            && periods.len() == 1
        {
            let p = &periods[0];
            return p.open.day == 0 && p.open.time == "0000" && p.close.is_none();
        }
        false
    }

    /// Returns the opening period for the given weekday (0 = Sunday, 6 = Saturday).
    fn period_for_day(&self, day: u8) -> Option<&Period> {
        self.opening_hours
            .as_ref()
            .and_then(|oh| oh.periods.as_ref())
            .and_then(|periods| {
                // The API can return multiple periods for a day, but the reference impl
                // assumes a single relevant period or just takes the one matching the day.
                periods.iter().find(|p| p.open.day == day)
            })
    }

    /// Formats the opening time for the date's day of the week.
    pub(super) fn opening_time(&self, date: OffsetDateTime) -> Option<String> {
        let weekday = date.weekday().number_days_from_sunday();
        let period = self.period_for_day(weekday)?;

        format_time_string(&period.open.time)
    }

    /// Formats the closing time for the date's day of the week.
    fn closing_time(&self, date: OffsetDateTime) -> Option<String> {
        let weekday = date.weekday().number_days_from_sunday();
        let period = self.period_for_day(weekday)?;

        period
            .close
            .as_ref()
            .and_then(|close| format_time_string(&close.time))
    }

    /// Formats the opening time for the date's day of the week, or a Danish placeholder when
    /// the API did not record one.
    pub(super) fn opening_time_or_unknown(&self, date: OffsetDateTime) -> String {
        self.opening_time(date)
            .unwrap_or_else(|| "ukendt tid".to_string())
    }

    /// Formats the closing time for the date's day of the week, or a Danish placeholder when
    /// the API did not record one.
    pub(super) fn closing_time_or_unknown(&self, date: OffsetDateTime) -> String {
        self.closing_time(date)
            .unwrap_or_else(|| "ukendt tid".to_string())
    }

    /// Returns (Open Time, Close Time) as `Time` objects for the requested date.
    pub(super) fn open_and_close_time(&self, date: OffsetDateTime) -> (Option<Time>, Option<Time>) {
        let weekday = date.weekday().number_days_from_sunday();

        self.period_for_day(weekday).map_or((None, None), |period| {
            let open = parse_hhmm(&period.open.time);
            let close = period.close.as_ref().and_then(|c| parse_hhmm(&c.time));

            (open, close)
        })
    }
}

const HHMM_FORMAT: &[FormatItem<'_>] = format_description!("[hour][minute]");

/// Parses a "HHMM" string into a `Time` object.
fn parse_hhmm(s: &str) -> Option<Time> {
    Time::parse(s, HHMM_FORMAT).ok()
}

/// Formats a `Time` as "HH:MM".
pub fn format_time(time: Time) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

/// Formats a "HHMM" string into "HH:MM".
fn format_time_string(s: &str) -> Option<String> {
    let time = parse_hhmm(s)?;

    Some(format_time(time))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hhmm() {
        let time = parse_hhmm("0930").unwrap();
        assert_eq!(time.hour(), 9);
        assert_eq!(time.minute(), 30);

        let time = parse_hhmm("1700").unwrap();
        assert_eq!(time.hour(), 17);
        assert_eq!(time.minute(), 0);

        assert!(parse_hhmm("2500").is_none());
    }

    #[test]
    fn test_format_time_string() {
        assert_eq!(format_time_string("0805"), Some("08:05".to_string()));
        assert_eq!(format_time_string("2359"), Some("23:59".to_string()));
        assert_eq!(format_time_string("invalid"), None);
    }

    #[test]
    fn test_place_details_is_always_open() {
        let json = r#"{
            "name": "7-Eleven",
            "opening_hours": {
                "open_now": true,
                "periods": [
                    { "open": { "day": 0, "time": "0000" } }
                ]
            }
        }"#;
        let place: PlaceDetails = serde_json::from_str(json).unwrap();
        assert!(place.is_always_open());
    }

    #[test]
    fn test_place_details_is_open_now() {
        let json = r#"{
            "name": "SuperBrugsen",
            "opening_hours": {
                "open_now": true,
                "periods": []
            }
        }"#;
        let place: PlaceDetails = serde_json::from_str(json).unwrap();
        assert!(place.is_open_now());
        assert!(!place.is_always_open());
    }
}
