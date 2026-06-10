use time::{Time, format_description::FormatItem, macros::format_description};

const HHMM_FORMAT: &[FormatItem<'_>] = format_description!("[hour][minute]");

/// Parses a "HHMM" string into a `Time` object.
pub(super) fn parse_hhmm(s: &str) -> Option<Time> {
    Time::parse(s, HHMM_FORMAT).ok()
}

/// Formats a "HHMM" string into "HH:MM".
pub(super) fn format_time_string(s: &str) -> Option<String> {
    let time = parse_hhmm(s)?;

    Some(format!("{:02}:{:02}", time.hour(), time.minute()))
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
}
