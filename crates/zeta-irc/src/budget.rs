//! Filling a listing up to the reply line budget.

/// The maximum length of a listing, leaving room for the sender prefix, `PRIVMSG` framing and
/// line ending overhead within the classic 512-byte IRC line limit.
pub const MAX_LISTING_LENGTH: usize = 400;

/// Appends `entries` to `message`, separated by `separator`, as long as the listing —
/// including `reserved` trailing bytes — stays within `budget` bytes.
///
/// The first entry is always appended, so a count in the message header is never misleading;
/// subsequent entries are only appended while they fit. Once an entry does not fit, no further
/// entries are appended and earlier ones are kept.
///
/// Returns the number of entries appended.
///
/// Callers that trail a fixed `suffix` after the listing pass its length as `reserved` and
/// append it themselves. Entries may be owned or borrowed strings.
pub fn append_entries_within_budget<S: AsRef<str>>(
    message: &mut String,
    entries: impl IntoIterator<Item = S>,
    separator: &str,
    budget: usize,
    reserved: usize,
) -> usize {
    let mut appended = 0usize;
    let mut first = true;

    for entry in entries {
        let entry = entry.as_ref();
        let prefix = if first { "" } else { separator };

        if !first && message.len() + prefix.len() + entry.len() + reserved > budget {
            break;
        }

        message.push_str(prefix);
        message.push_str(entry);
        first = false;
        appended += 1;
    }

    appended
}

/// Appends `entries` to `message` like [`append_entries_within_budget`], adding the suffix
/// produced by `more` when not every entry fits.
///
/// `total` is the number of entries before budgeting, and `more` receives the number that were
/// left out. `reserved` covers suffixes the caller adds besides the truncation suffix, as with
/// [`append_entries_within_budget`]; the truncation suffix itself is measured here, so callers
/// cannot drift from the length `more` actually returns. Returns the number of entries
/// appended.
pub fn append_entries_with_more<S: AsRef<str>>(
    message: &mut String,
    entries: impl IntoIterator<Item = S>,
    total: usize,
    separator: &str,
    budget: usize,
    reserved: usize,
    more: impl Fn(usize) -> String,
) -> usize {
    // Reserve for the longest suffix, when every entry is dropped: over-reserving by a few
    // bytes for smaller counts is preferable to a suffix that pushes the line over budget.
    // An empty listing reserves nothing.
    let suffix_reserve = if total == 0 { 0 } else { more(total).len() };

    let appended = append_entries_within_budget(
        message,
        entries,
        separator,
        budget,
        reserved + suffix_reserve,
    );
    let dropped = total.saturating_sub(appended);

    if dropped > 0 {
        message.push_str(&more(dropped));
    }

    appended
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_entries_within_the_budget() {
        let mut message = String::from("header: ");

        let appended = append_entries_within_budget(&mut message, ["a", "b"], ", ", 30, 0);

        assert_eq!(appended, 2);
        assert_eq!(message, "header: a, b");
    }

    #[test]
    fn always_appends_the_first_entry_and_keeps_prefixes() {
        let mut message = String::from("header: ");

        let appended =
            append_entries_within_budget(&mut message, ["first", "second", "third"], ", ", 20, 0);

        // The first entry is unconditional; "header: first, second" is 21 bytes and would
        // exceed the 20-byte budget, so the loop stops after the first entry.
        assert_eq!(appended, 1);
        assert_eq!(message, "header: first");
    }

    #[test]
    fn reserves_room_for_a_suffix() {
        let mut message = String::new();

        let appended = append_entries_within_budget(&mut message, ["first", "second"], ", ", 10, 8);

        // "first" is unconditional; "second" (6 bytes) plus the separator and the 8 reserved
        // bytes exceeds the budget.
        assert_eq!(appended, 1);
        assert_eq!(message, "first");
    }

    #[test]
    fn appends_a_suffix_for_the_entries_that_did_not_fit() {
        let mut message = String::new();

        let appended = append_entries_with_more(
            &mut message,
            ["first", "second", "third"],
            3,
            ", ",
            25,
            0,
            |dropped| format!(" (+{dropped} more)"),
        );

        assert_eq!(appended, 2);
        assert_eq!(message, "first, second (+1 more)");
    }

    #[test]
    fn the_truncation_suffix_counts_against_the_budget() {
        let mut message = String::new();

        let appended =
            append_entries_with_more(&mut message, ["first", "second"], 2, ", ", 20, 0, |_| {
                " (1 more)".to_string()
            });

        // "first, second" fits on its own, but the 9-byte suffix that would follow does not.
        assert_eq!(appended, 1);
        assert_eq!(message, "first (1 more)");
        assert!(message.len() <= 20);
    }

    #[test]
    fn does_not_append_the_suffix_when_every_entry_fits() {
        let mut message = String::new();

        // The suffix length is measured up front, even when every entry ends up fitting.
        let appended =
            append_entries_with_more(&mut message, ["first", "second"], 2, ", ", 40, 0, |_| {
                " (0 more)".to_string()
            });

        assert_eq!(appended, 2);
        assert_eq!(message, "first, second");
    }
}
