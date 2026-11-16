//! Packing help lists into messages within the IRC message budget.

use zeta_plugin::irc;

use super::{MAX_MESSAGE_LENGTH, SEPARATOR};

/// Prepends the packed list of `entries` to `header`.
///
/// Lists longer than [`MAX_MESSAGE_LENGTH`](super::MAX_MESSAGE_LENGTH) are split at entry
/// boundaries; the continuation messages carry the remaining entries without the header. The
/// header counts against the budget of the first message.
pub(super) fn with_list<S: AsRef<str>>(header: &str, entries: &[S]) -> Vec<String> {
    let budget = MAX_MESSAGE_LENGTH.saturating_sub(header.len() + 1);
    let lists = pack(entries, budget);

    if lists.is_empty() {
        return vec![header.to_owned()];
    }

    std::iter::once(format!("{header} {}", lists[0]))
        .chain(lists.into_iter().skip(1))
        .collect()
}

/// Packs `entries` into as few messages as possible without exceeding `max_length` or splitting
/// an entry.
pub(super) fn pack<S: AsRef<str>>(entries: &[S], max_length: usize) -> Vec<String> {
    let mut messages = Vec::new();
    let mut remaining = entries;

    while !remaining.is_empty() {
        // The message cannot exceed the budget, and reserving all of it would over-allocate
        // every short message; a capped initial capacity keeps the common listings from
        // reallocating without reserving thousands of bytes for a handful of entries.
        let mut message = String::with_capacity(max_length.min(512));
        let appended = irc::append_entries_within_budget(
            &mut message,
            remaining.iter(),
            SEPARATOR,
            max_length,
            0,
        );

        messages.push(message);
        remaining = &remaining[appended..];
    }

    messages
}
