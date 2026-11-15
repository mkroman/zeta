//! Parsing ISO 8601 duration strings.

use std::time::Duration;

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
