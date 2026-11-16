//! Reply formatting for the `.filter` command.
//!
//! Composes the labeled confirmation of added and removed filters and the filter listings shown
//! by `list` and `delete`. Every filter is described by its id, criteria and channel scope, and
//! every listing stays within the IRC line budget.

use std::fmt::Write as _;

use zeta_plugin::irc::labeled_reply;
use zeta_plugin::irc::{self, MAX_LISTING_LENGTH};

use crate::model::Filter;

/// The complete labeled confirmation for the added or removed `filters`, describing each one so
/// the sender can act on the ids without a follow-up `.filter list`. Callers send the result
/// as-is.
pub(crate) fn changed_response(name: &str, verb: &str, filters: &[Filter]) -> String {
    match filters {
        [filter] => labeled_reply(name, format_args!("{verb} {}.", describe(filter))),
        // More than one filter, so the plural is always right.
        filters => listing(
            labeled_reply(name, format_args!("{verb} {} filters: ", filters.len())),
            filters,
            0,
        ),
    }
}

/// Appends the described `filters` to `header`, within the line budget and leaving room for a
/// suffix of `reserved` bytes. A listing that does not fit entirely gains a ` (+N more)`
/// suffix naming how many filters were left out.
pub(crate) fn listing(header: String, filters: &[Filter], reserved: usize) -> String {
    let mut listing = header;

    irc::append_entries_with_more(
        &mut listing,
        filters.iter().map(describe),
        filters.len(),
        ", ",
        MAX_LISTING_LENGTH,
        reserved,
        |dropped| format!(" (+{dropped} more)"),
    );

    listing
}

/// Formats a filter for listings.
pub(crate) fn describe(filter: &Filter) -> String {
    let criteria = [
        ("host", filter.host.as_deref()),
        ("path", filter.path.as_deref()),
        ("nick", filter.nickname.as_deref()),
        ("user", filter.username.as_deref()),
        ("hostname", filter.hostname.as_deref()),
    ];
    // Every set criterion contributes its `name=value` and a separating space; the sum is an
    // upper bound on the composed length, so the buffer is allocated once and never regrows.
    let mut text = String::with_capacity(
        criteria
            .iter()
            .filter_map(|&(name, value)| value.map(|value| name.len() + value.len() + 2))
            .sum(),
    );

    for &(name, value) in &criteria {
        let Some(value) = value else {
            continue;
        };

        if !text.is_empty() {
            text.push(' ');
        }

        let _ = write!(text, "{name}={value}");
    }

    let scope = filter.channel.as_deref().unwrap_or("all channels");

    format!("#{}: {text} ({scope})", filter.id)
}
