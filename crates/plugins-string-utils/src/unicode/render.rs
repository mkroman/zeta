//! Rendering one `.uni` entry's codepoints, name and category within the entry budget.

use std::fmt::Write as _;

use zeta_plugin::irc::{BOLD, COLOR, RESET};

use super::ENTRY_BUDGET;

/// The bytes that open an entry's codepoint list: `{` followed by the reset and bold codes.
pub(super) const ENTRY_OPEN: &str = "{\x0f\x02";
/// Appended when a listing does not fit the budget.
pub(super) const SUFFIX: &str = ", …";
/// The widest a code point can be spelled, used to prove an entry always fits.
pub(super) const WIDEST_POINT: &str = "U+10FFFF";
/// The bytes of an entry's tail around the name and category.
pub(super) const TAIL_FIXED_LEN: usize = BOLD.len()
    + COLOR.len()
    + "}:".len()
    + RESET.len()
    + " ".len()
    + COLOR.len()
    + " (".len()
    + RESET.len()
    + COLOR.len()
    + ")".len();

/// Appends the tail that follows an entry's codepoint list.
fn write_entry_tail(out: &mut String, name: &str, category: &str) {
    let _ = write!(
        out,
        "{BOLD}{COLOR}}}:{RESET} {name}{COLOR} ({RESET}{category}{COLOR})"
    );
}

/// The bytes [`write_entry_tail`] appends after the codepoint list.
const fn entry_tail_len(name: &str, category: &str) -> usize {
    TAIL_FIXED_LEN
        .saturating_add(name.len())
        .saturating_add(category.len())
}

/// The bytes [`write_entry_tail`] appends after the codepoint list.
#[cfg(test)]
pub(super) fn entry_tail(name: &str, category: &str) -> String {
    let mut tail = String::with_capacity(entry_tail_len(name, category));
    write_entry_tail(&mut tail, name, category);
    tail
}

/// Renders an entry into `entry`: its codepoints bold inside the braces, then name and category.
///
/// The codepoint list is a listing too: canonicalization strips every `U+FE0F`, so the input
/// that matched can be much longer than the stored key, and the list stops with [`SUFFIX`]
/// inside the braces. [`entry_tail_len`] is the exact length of the tail that follows the list,
/// so the reserved budget cannot drift from what is written.
pub(super) fn render_entry_into(points: &str, name: &str, category: &str, entry: &mut String) {
    let tail_len = entry_tail_len(name, category);
    entry.clear();
    entry.push_str(ENTRY_OPEN);

    // The first codepoint is appended unconditionally, so the entry only fits while the tail
    // leaves room for the prefix and `U+10FFFF` on top of the reserved bytes.
    debug_assert!(
        ENTRY_OPEN.len() + WIDEST_POINT.len() + SUFFIX.len() + tail_len <= ENTRY_BUDGET,
        "the name {name:?} does not leave room for the codepoint list"
    );

    append_points(entry, points, tail_len);
    write_entry_tail(entry, name, category);
}

/// Appends the codepoint list within [`ENTRY_BUDGET`], reserving `tail_len` for the tail.
///
/// The first codepoint is appended unconditionally; each later one is written and rolled back
/// when it would push the entry past the budget, which is the same boundary
/// `append_entries_with_more` uses without building a `String` per codepoint.
fn append_points(entry: &mut String, points: &str, tail_len: usize) {
    let total = points.chars().count();
    let mut appended = 0usize;

    for c in points.chars() {
        let before = entry.len();
        if appended > 0 {
            entry.push_str(", ");
        }
        let _ = write!(entry, "U+{:04X}", u32::from(c));

        if appended > 0 && entry.len() + tail_len + SUFFIX.len() > ENTRY_BUDGET {
            entry.truncate(before);
            break;
        }
        appended += 1;
    }

    if appended < total {
        entry.push_str(SUFFIX);
    }
}

/// Renders an entry as a standalone string.
#[cfg(test)]
pub(super) fn render_entry(points: &str, name: &str, category: &str) -> String {
    let mut entry = String::with_capacity(ENTRY_BUDGET);
    render_entry_into(points, name, category, &mut entry);
    entry
}
