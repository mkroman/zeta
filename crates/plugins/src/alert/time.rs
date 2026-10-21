//! Parsing of the alert datetime expression.

use chrono::Days;
use interim::{Dialect, parse_date_string};
use sqlx::types::chrono::{DateTime, Local, Utc};

/// Errors that can occur while parsing the datetime of an alert.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseTimeError {
    /// The datetime could not be understood.
    #[error("ambiguous or unsupported datetime")]
    Ambiguous,
    /// The datetime occurs in the past.
    #[error("specified time occurs in the past")]
    Past,
}

/// Splits the command arguments into the alert message and its datetime expression.
///
/// The message and datetime are separated by the last occurrence of `at` or `in`, so messages
/// containing either word are handled correctly. Returns `None` if either part is missing.
#[must_use]
pub(super) fn split_args(args: &str) -> Option<(&str, &str)> {
    let index = rfind_ignore_case(args, " at ").max(rfind_ignore_case(args, " in "))?;

    let message = &args[..index];
    let time_spec = args[index + " at ".len()..].trim();

    (!message.trim().is_empty() && !time_spec.is_empty()).then_some((message, time_spec))
}

/// Returns the byte index of the last occurrence of `needle` in `s`, ignoring ASCII case.
fn rfind_ignore_case(s: &str, needle: &str) -> Option<usize> {
    s.as_bytes()
        .windows(needle.len())
        .rposition(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

/// Parses the datetime expression of an alert.
///
/// The expression is parsed as a human datetime relative to `now`; if a bare time ("4:20")
/// resolves to the past it is rolled forward to the next day, while expressions containing a
/// date that occurs in the past are rejected.
///
/// # Errors
///
/// Returns [`ParseTimeError::Ambiguous`] if the expression could not be parsed, or
/// [`ParseTimeError::Past`] if the expression contains a date that occurs in the past.
pub(super) fn parse_time(
    spec: &str,
    now: DateTime<Local>,
) -> Result<DateTime<Utc>, ParseTimeError> {
    // Tolerate a leading article in the datetime expression, e.g. "at the weekend".
    let spec = strip_article(spec.trim());

    let time: DateTime<Local> = parse_date_string(spec, now, Dialect::Uk)
        .ok()
        .or_else(|| parse_time_first(spec, now))
        .ok_or(ParseTimeError::Ambiguous)?;

    if time > now {
        return Ok(time.with_timezone(&Utc));
    }

    if is_time_only(spec) {
        // Bare times such as "4:20" roll forward to their next occurrence.
        let days = u64::try_from((now - time).num_days()).unwrap_or_default() + 1;
        let time = time
            .checked_add_days(Days::new(days))
            .ok_or(ParseTimeError::Ambiguous)?;

        return Ok(time.with_timezone(&Utc));
    }

    Err(ParseTimeError::Past)
}

/// Strips leading `at` or `on` article from the datetime expression.
fn strip_article(spec: &str) -> &str {
    for article in ["at ", "on "] {
        if spec.len() > article.len()
            && spec
                .get(..article.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(article))
        {
            // The article is ASCII, so the prefix ends at a character boundary.
            return &spec[article.len()..];
        }
    }

    spec
}

/// Parses datetime expressions written time-first, e.g. `10:30 12/12/2032`, by swapping the two
/// tokens and letting the date lead as the parser expects.
///
/// Only applies when the first token is a clock time and the second looks like a calendar date;
/// otherwise returns `None`.
fn parse_time_first(spec: &str, now: DateTime<Local>) -> Option<DateTime<Local>> {
    let (time, date) = spec.split_once(' ')?;

    if !is_clock_time(time) || !(date.contains('/') || date.contains('-')) {
        return None;
    }

    parse_date_string(&format!("{date} {time}"), now, Dialect::Uk).ok()
}

/// Returns `true` if the datetime expression consists solely of a clock time, e.g. `4:20`,
/// `16:34:10` or `4:20pm`.
fn is_time_only(spec: &str) -> bool {
    // Split off a separate meridiem token, e.g. the "pm" of "4:20 pm".
    let spec = match spec.split_once(' ') {
        Some((time, meridiem))
            if meridiem.eq_ignore_ascii_case("am") || meridiem.eq_ignore_ascii_case("pm") =>
        {
            time
        }
        _ => spec,
    };

    // Strip a glued-on meridiem, e.g. the "pm" of "4:20pm".
    let spec = {
        let lower = spec.to_ascii_lowercase();

        ["pm", "am"]
            .iter()
            .find(|suffix| lower.ends_with(*suffix))
            .map_or(spec, |suffix| &spec[..spec.len() - suffix.len()])
    };

    is_clock_time(spec)
}

/// Returns `true` if `s` is a clock time on the form `H:M` or `H:M:S`, e.g. `4:20` or `16:34:10`.
fn is_clock_time(s: &str) -> bool {
    let segments = s.split(':').count();

    (segments == 2 || segments == 3)
        && s.split(':')
            .all(|segment| !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use sqlx::types::chrono::TimeZone;

    use super::*;

    #[test]
    fn splits_message_and_datetime() {
        assert_eq!(
            split_args("hello world at 4:20"),
            Some(("hello world", "4:20"))
        );
        assert_eq!(
            split_args("some longer message at 10:30 12/12/2032"),
            Some(("some longer message", "10:30 12/12/2032"))
        );
        assert_eq!(
            split_args("another message at 12/12/2032 10:30"),
            Some(("another message", "12/12/2032 10:30"))
        );
    }

    #[test]
    fn splits_relative_durations() {
        assert_eq!(
            split_args("check the oven in 10 minutes"),
            Some(("check the oven", "10 minutes"))
        );
    }

    #[test]
    fn splits_on_the_last_tense_marker() {
        assert_eq!(
            split_args("meet me at the station at 4:20"),
            Some(("meet me at the station", "4:20"))
        );
        assert_eq!(
            split_args("come in in 10 minutes"),
            Some(("come in", "10 minutes"))
        );
    }

    #[test]
    fn rejects_arguments_without_a_message_or_datetime() {
        assert_eq!(split_args(""), None);
        assert_eq!(split_args("at 4:20"), None);
        assert_eq!(split_args("hello world"), None);
        assert_eq!(split_args("hello world at"), None);
        assert_eq!(split_args("hello world in "), None);
        assert_eq!(split_args(" at 4:20"), None);
    }

    #[test]
    fn is_case_insensitive() {
        assert_eq!(
            split_args("Hello World AT 4:20"),
            Some(("Hello World", "4:20"))
        );
        assert_eq!(
            split_args("hello world IN 10 minutes"),
            Some(("hello world", "10 minutes"))
        );
    }

    #[test]
    fn parses_future_datetimes() {
        let now = Local::now();

        let time = parse_time("12/12/2032 10:30", now).unwrap();
        let expected = Local
            .with_ymd_and_hms(2032, 12, 12, 10, 30, 0)
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(time, expected);
    }

    #[test]
    fn parses_month_before_day() {
        let now = Local::now();

        let time = parse_time("10:30 12/12/2032", now).unwrap();
        let expected = Local
            .with_ymd_and_hms(2032, 12, 12, 10, 30, 0)
            .unwrap()
            .with_timezone(&Utc);

        assert_eq!(time, expected);
    }

    #[test]
    fn parses_relative_durations() {
        let now = Local::now();

        let time = parse_time("10 minutes", now).unwrap();
        let local = time.with_timezone(&Local);

        assert!(time > now.with_timezone(&Utc));
        assert!(local - now < chrono::TimeDelta::try_minutes(11).unwrap());
        assert!(local - now > chrono::TimeDelta::try_minutes(9).unwrap());
    }

    #[test]
    fn rolls_bare_times_forward_to_the_next_day() {
        let now = Local::now();

        let time = parse_time("4:20", now).unwrap();
        let local = time.with_timezone(&Local);

        assert!(time > now.with_timezone(&Utc));
        assert_eq!(local.format("%H:%M").to_string(), "04:20");
    }

    #[test]
    fn rejects_past_dates() {
        let now = Local::now();

        assert_eq!(
            parse_time("1/1/2000", now).unwrap_err(),
            ParseTimeError::Past
        );
    }

    #[test]
    fn rejects_unparseable_datetimes() {
        let now = Local::now();

        assert_eq!(
            parse_time("not a datetime", now).unwrap_err(),
            ParseTimeError::Ambiguous
        );
    }

    #[test]
    fn tolerates_a_leading_article() {
        let now = Local::now();

        let time = parse_time("at 4:20", now).unwrap();

        assert!(time > now.with_timezone(&Utc));
    }

    #[test]
    fn detects_bare_times() {
        assert!(is_time_only("4:20"));
        assert!(is_time_only("16:34"));
        assert!(is_time_only("16:34:10"));
        assert!(is_time_only("4:20 pm"));
        assert!(is_time_only("4:20pm"));

        assert!(!is_time_only("12/12/2032 10:30"));
        assert!(!is_time_only("10:30 12/12/2032"));
        assert!(!is_time_only("tomorrow 10:30"));
        assert!(!is_time_only("next friday 8pm"));
        assert!(!is_time_only("10 minutes"));
    }
}
