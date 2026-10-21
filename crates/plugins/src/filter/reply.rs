//! Replies that describe filters.

use std::fmt::Write as _;

use zeta_utils::utils::{MAX_LISTING_LENGTH, append_entries_within_budget};

use super::{NAME, model::FilterRule};
use crate::prelude::*;

/// Formats `count` with the pluralized `noun`, e.g. `1 filter` or `3 filters`.
pub(super) fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// Room for a ` (+N more)` truncation suffix on a listing, covering counts up to five digits.
const MORE_SUFFIX_RESERVE: usize = " (+99999 more)".len();

/// The confirmation for the added or removed `filters`, describing each one so the sender can
/// act on the ids without a follow-up `.filter list`.
pub(super) fn changed_response(verb: &str, filters: &[FilterRule]) -> String {
    match filters {
        [filter] => reply(NAME, format!("{verb} {}.", describe(filter))),
        // More than one filter, so the plural is always right.
        filters => listing(
            reply(NAME, format!("{verb} {} filters: ", filters.len())),
            filters,
            0,
        ),
    }
}

/// Appends the described `filters` to `header`, within the line budget and leaving room for a
/// suffix of `reserved` bytes. A listing that does not fit entirely gains a ` (+N more)`
/// suffix naming how many filters were left out.
pub(super) fn listing(header: String, filters: &[FilterRule], reserved: usize) -> String {
    let mut listing = header;

    let appended = append_entries_within_budget(
        &mut listing,
        filters.iter().map(describe),
        ", ",
        MAX_LISTING_LENGTH,
        reserved + MORE_SUFFIX_RESERVE,
    );

    let dropped = filters.len() - appended;

    if dropped > 0 {
        let _ = write!(listing, " (+{dropped} more)");
    }

    listing
}

/// Formats a filter for listings.
pub(super) fn describe(filter: &FilterRule) -> String {
    let criteria = [
        ("host", filter.host.as_deref()),
        ("path", filter.path.as_deref()),
        ("nick", filter.nickname.as_deref()),
        ("user", filter.username.as_deref()),
        ("hostname", filter.hostname.as_deref()),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|value| format!("{name}={value}")))
    .collect::<Vec<_>>()
    .join(" ");

    let scope = filter.channel.as_deref().unwrap_or("all channels");

    format!("#{}: {criteria} ({scope})", filter.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_added_filter_is_described_with_its_id() {
        let filter = FilterRule {
            id: 12,
            channel: Some("#foo".into()),
            host: Some("example.com".into()),
            path: None,
            nickname: None,
            username: None,
            hostname: None,
        };

        assert_eq!(
            changed_response("Added", &[filter]),
            reply(NAME, "Added #12: host=example.com (#foo).")
        );
    }

    #[test]
    fn multiple_added_filters_are_listed_with_a_count() {
        let first = FilterRule {
            id: 12,
            channel: None,
            host: Some("a.com".into()),
            path: Some("/x/*".into()),
            nickname: None,
            username: None,
            hostname: None,
        };
        let second = FilterRule {
            id: 13,
            channel: None,
            host: Some("b.com".into()),
            path: Some("/x/*".into()),
            nickname: None,
            username: None,
            hostname: None,
        };

        assert_eq!(
            changed_response("Added", &[first, second]),
            reply(
                NAME,
                "Added 2 filters: #12: host=a.com path=/x/* (all channels), \
                 #13: host=b.com path=/x/* (all channels)"
            )
        );
    }

    #[test]
    fn counts_are_pluralized() {
        assert_eq!(plural(1, "filter"), "1 filter");
        assert_eq!(plural(2, "filter"), "2 filters");
        assert_eq!(plural(0, "filter"), "0 filters");
    }

    #[test]
    fn a_truncated_listing_names_the_dropped_filters() {
        let long = FilterRule {
            id: 1,
            channel: None,
            host: Some(format!("long.example/{}", "x".repeat(180))),
            path: None,
            nickname: None,
            username: None,
            hostname: None,
        };
        let second = FilterRule {
            id: 2,
            ..long.clone()
        };

        let message = listing(reply(NAME, "2 filters: "), &[long, second], 0);

        assert!(message.len() <= MAX_LISTING_LENGTH);
        assert!(message.contains("#1: "), "{message}");
        assert!(message.ends_with(" (+1 more)"), "{message}");
    }

    #[test]
    fn describes_filters() {
        let filter = FilterRule {
            id: 12,
            channel: Some("#foo".into()),
            host: Some("*.com".into()),
            path: None,
            nickname: None,
            username: Some("*other".into()),
            hostname: None,
        };

        assert_eq!(describe(&filter), "#12: host=*.com user=*other (#foo)");

        let global = FilterRule {
            id: 3,
            channel: None,
            host: None,
            path: Some("/title/*".into()),
            nickname: None,
            username: None,
            hostname: None,
        };

        assert_eq!(describe(&global), "#3: path=/title/* (all channels)");
    }

    #[test]
    fn listings_stay_within_the_budget() {
        let mut message = reply(NAME, "2 filters matching your criteria: ");

        let appended = append_entries_within_budget(
            &mut message,
            [
                format!("long entry {}", "x".repeat(180)),
                format!("long entry {}", "y".repeat(180)),
            ],
            ", ",
            MAX_LISTING_LENGTH,
            0,
        );

        assert_eq!(appended, 1);
        assert!(message.len() <= MAX_LISTING_LENGTH);
        assert!(message.contains("long entry xxx"));
    }
}
