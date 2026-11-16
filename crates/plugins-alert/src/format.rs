//! Reply messages and listings for the `.alert` command.

use chrono::Datelike;
use rand::prelude::IteratorRandom;
use sqlx::types::chrono::{DateTime, Local};

use zeta_plugin::irc::{self, MAX_LISTING_LENGTH};
use zeta_plugin::irc::{COLOR, RESET, labeled_reply, unstyle_text};
use zeta_utils::text::TruncatableExt;

use super::model::Alert;

/// Reply messages used when an alert has been stored.
const SUCCESS_MESSAGES: &[&str] = &[
    "Got it, boss!",
    "Will do.",
    "At your service.",
    "Gotcha.",
    "Done and done.",
    "Roger.",
    "As you wish.",
    "Your wish is my command.",
    "Very well, then.",
];

/// Returns a random success message.
pub(crate) fn success_message() -> &'static str {
    SUCCESS_MESSAGES
        .iter()
        .choose(&mut rand::rng())
        .unwrap_or(&SUCCESS_MESSAGES[0])
}

/// The maximum number of pending alerts included in a listing.
const MAX_LISTED_ALERTS: usize = 3;

/// The maximum number of characters of an alert message included in a listing.
const MAX_LISTED_MESSAGE_CHARS: usize = 50;

/// Formats the reply to `.alert -l` for `pending`, ordered by the time the alerts are due.
///
/// At most [`MAX_LISTED_ALERTS`] alerts are included, and entries are only appended while the
/// listing stays within [`MAX_LISTING_LENGTH`]; the count in the header always reflects every
/// pending alert.
pub(crate) fn format_pending(pending: &[Alert]) -> String {
    if pending.is_empty() {
        return labeled_reply("Alert", "You have no pending alerts");
    }

    let mut listing = labeled_reply(
        "Alert",
        format_args!("Pending alerts:\x0f {}", pending.len()),
    );

    let entries = pending
        .iter()
        .take(MAX_LISTED_ALERTS)
        .enumerate()
        .map(|(index, alert)| match index {
            0 => format!("{COLOR} Next up: {}", format_entry(alert)),
            _ => format_entry(alert),
        });

    irc::append_entries_within_budget(
        &mut listing,
        entries,
        &format!("{COLOR}, then: "),
        MAX_LISTING_LENGTH,
        0,
    );

    listing
}

/// Formats a pending alert as a `“message” Mon DDth HH:MM` entry.
fn format_entry(alert: &Alert) -> String {
    let message = unstyle_text(&alert.message);
    let message = message.truncate_with_suffix(MAX_LISTED_MESSAGE_CHARS, "…");
    let due = format_due(alert.time.with_timezone(&Local));

    format!("“{RESET}{message}{COLOR}”{RESET} {due}{COLOR}")
}

/// Formats the due time of an alert as `Sep 16th 11:21`.
fn format_due(time: DateTime<Local>) -> String {
    let day = time.day();

    format!(
        "{} {day}{} {}",
        time.format("%b"),
        ordinal_suffix(day),
        time.format("%H:%M")
    )
}

/// Returns the ordinal suffix of `day`, e.g. `st` for 1 and 21, `th` for 11 through 13.
const fn ordinal_suffix(day: u32) -> &'static str {
    if matches!(day % 100, 11..=13) {
        return "th";
    }

    match day % 10 {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    }
}

#[cfg(test)]
mod tests {
    use sqlx::types::chrono::{TimeZone, Utc};

    use super::*;

    /// Returns a pending alert fixture.
    fn alert(message: &str, time: DateTime<Utc>) -> Alert {
        Alert {
            id: 1,
            nickname: "smoke".into(),
            channel: "#smoke".into(),
            message: message.into(),
            time,
        }
    }

    /// Returns the UTC equivalent of a local wall-clock time.
    fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
        Local
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn ordinals() {
        assert_eq!(ordinal_suffix(1), "st");
        assert_eq!(ordinal_suffix(2), "nd");
        assert_eq!(ordinal_suffix(3), "rd");
        assert_eq!(ordinal_suffix(4), "th");
        assert_eq!(ordinal_suffix(11), "th");
        assert_eq!(ordinal_suffix(12), "th");
        assert_eq!(ordinal_suffix(13), "th");
        assert_eq!(ordinal_suffix(21), "st");
        assert_eq!(ordinal_suffix(22), "nd");
        assert_eq!(ordinal_suffix(23), "rd");
        assert_eq!(ordinal_suffix(31), "st");
    }

    #[test]
    fn formats_due_times() {
        assert_eq!(
            format_due(Local.with_ymd_and_hms(2026, 9, 16, 11, 21, 0).unwrap()),
            "Sep 16th 11:21"
        );
        assert_eq!(
            format_due(Local.with_ymd_and_hms(2026, 1, 1, 0, 5, 0).unwrap()),
            "Jan 1st 00:05"
        );
        assert_eq!(
            format_due(Local.with_ymd_and_hms(2026, 3, 13, 23, 59, 0).unwrap()),
            "Mar 13th 23:59"
        );
    }

    #[test]
    fn formats_no_pending_alerts() {
        assert_eq!(
            irc::strip_formatting(&format_pending(&[])),
            "> Alert: You have no pending alerts"
        );
    }

    #[test]
    fn formats_a_single_pending_alert() {
        let pending = [alert("hello world", at(2026, 9, 16, 11, 21))];

        // The exact layout pin for the formatter.
        assert_eq!(
            format_pending(&pending),
            concat!(
                "\x0310>\x0f\x02 Alert:\x02\x0310 Pending alerts:\x0f 1",
                "\x0310 Next up: “\x0fhello world\x0310”\x0f Sep 16th 11:21\x0310",
            )
        );
    }

    #[test]
    fn formats_at_most_three_pending_alerts() {
        let pending = [
            alert("first", at(2026, 9, 16, 11, 21)),
            alert("second", at(2026, 9, 16, 11, 22)),
            alert("third", at(2026, 9, 16, 11, 23)),
            alert("fourth", at(2026, 9, 17, 9, 15)),
        ];
        let listing = irc::strip_formatting(&format_pending(&pending));

        assert!(
            listing.starts_with("> Alert: Pending alerts: 4"),
            "{listing}"
        );
        assert!(
            listing.contains("Next up: “first” Sep 16th 11:21"),
            "{listing}"
        );
        assert!(
            listing.contains("then: “second” Sep 16th 11:22"),
            "{listing}"
        );
        assert!(
            listing.contains("then: “third” Sep 16th 11:23"),
            "{listing}"
        );
        assert!(!listing.contains("fourth"), "{listing}");
    }

    #[test]
    fn truncates_long_messages() {
        let pending = [alert(&"a".repeat(80), at(2026, 9, 16, 11, 21))];
        let listing = irc::strip_formatting(&format_pending(&pending));

        assert!(
            listing.contains(&format!("Next up: “{}…” Sep 16th 11:21", "a".repeat(50))),
            "{listing}"
        );
        assert!(!listing.contains(&"a".repeat(51)), "{listing}");
    }

    #[test]
    fn strips_formatting_from_stored_messages() {
        let pending = [alert("\x0310fake\nmessage\x02", at(2026, 9, 16, 11, 21))];
        let listing = irc::strip_formatting(&format_pending(&pending));

        assert!(
            listing.contains("Next up: “fake message” Sep 16th 11:21"),
            "{listing}"
        );
    }

    #[test]
    fn omits_entries_beyond_the_listing_budget() {
        // Each 50-character message is 100 bytes in UTF-8, so the third entry would push the
        // listing past the message length budget.
        let message = "æ".repeat(50);
        let pending = [
            alert(&message, at(2026, 9, 16, 11, 21)),
            alert(&message, at(2026, 9, 16, 11, 22)),
            alert(&message, at(2026, 9, 16, 11, 23)),
        ];

        let listing = format_pending(&pending);

        assert!(listing.len() <= MAX_LISTING_LENGTH);
        assert_eq!(listing.matches("Next up").count(), 1);
        assert_eq!(listing.matches("then:").count(), 1);
    }
}
