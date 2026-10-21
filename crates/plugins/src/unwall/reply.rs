//! Replies describing the covered sites and their statistics.

use std::fmt::Write as _;

use zeta_utils::duration::TimeInWords;
use zeta_utils::utils::{MAX_LISTING_LENGTH, append_entries_within_budget};

use super::{
    NAME,
    model::{HostStatistics, Site, Statistics},
};
use crate::prelude::*;

/// Joins `items` with cyan commas and a final cyan `and`, each item emphasized in the default
/// color: `\x0fa\x0310, \x0fb\x0310 and\x0f c`.
pub(super) fn join_and(items: &[String]) -> String {
    let mut joined = String::new();

    for (index, item) in items.iter().enumerate() {
        if index == 0 {
            let _ = write!(joined, "{RESET}{item}");
        } else if index + 1 == items.len() {
            let _ = write!(joined, "{REPLY_COLOR} and{RESET} {item}");
        } else {
            let _ = write!(joined, "{REPLY_COLOR},{RESET} {item}");
        }
    }

    joined
}

/// Summarizes `sites` in a single line, or explains how to add the first one.
pub(super) fn list_reply(sites: &[Site]) -> String {
    if sites.is_empty() {
        return reply(
            NAME,
            format!(
                "{RESET}No unwalled sites yet{REPLY_COLOR}: add one with\
                 {RESET} .unwall add <host>{REPLY_COLOR}, e.g.{RESET} .unwall add bloomberg.com{REPLY_COLOR}."
            ),
        );
    }

    let mut message = reply(
        NAME,
        format!(
            "{RESET}{}{REPLY_COLOR} unwalled sites:{RESET} ",
            sites.len()
        ),
    );

    let reserved = " (999 more)".len();
    let appended = append_entries_within_budget(
        &mut message,
        sites.iter().map(|site| site.host.as_str()),
        &format!("{REPLY_COLOR}, {RESET}"),
        MAX_LISTING_LENGTH,
        reserved,
    );

    if appended < sites.len() {
        let _ = write!(message, "{REPLY_COLOR} ({} more)", sites.len() - appended);
    }

    message
}

/// Summarizes the overall statistics of the unwalled links.
pub(super) fn stats_reply(stats: &Statistics) -> String {
    reply(
        NAME,
        format!(
            "{RESET}{}{REPLY_COLOR} links unwalled across{RESET} {}{REPLY_COLOR} sites \
             by{RESET} {}{REPLY_COLOR} users ({RESET}{}{REPLY_COLOR} today).",
            stats.urls, stats.hosts, stats.users, stats.urls_today
        ),
    )
}

/// Summarizes the statistics of one `host`.
pub(super) fn info_reply(host: &str, stats: Option<&HostStatistics>) -> String {
    stats.map_or_else(
        || {
            reply(
                NAME,
                format!("No unwalled links for{RESET} {host}{REPLY_COLOR}."),
            )
        },
        |stats| {
            reply(
                NAME,
                format!(
                    "{RESET}{}{REPLY_COLOR} links unwalled for{RESET} {host}{REPLY_COLOR} \
                     by{RESET} {}{REPLY_COLOR} users ({RESET}{}{REPLY_COLOR} today, most recent\
                     {RESET} {}{REPLY_COLOR}).",
                    stats.urls,
                    stats.users,
                    stats.urls_today,
                    stats.latest.map_or_default(|latest| latest.time_ago()),
                ),
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_with_a_final_and() {
        assert_eq!(join_and(&[]), "");
        assert_eq!(join_and(&["a".into()]), format!("{RESET}a"));
        assert_eq!(
            join_and(&["a".into(), "b".into()]),
            format!("{RESET}a{REPLY_COLOR} and{RESET} b")
        );
        assert_eq!(
            join_and(&["a".into(), "b".into(), "c".into()]),
            format!("{RESET}a{REPLY_COLOR},{RESET} b{REPLY_COLOR} and{RESET} c")
        );
    }
}
