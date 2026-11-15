//! The `.uni` renderer: names, general categories and grouped emoji sequences.
//!
//! The renderer keeps each entry cheap until the byte budget decides whether to render it,
//! because one padded emoji sequence can exhaust a whole IRC line on its own.
//!
//! The module is split by concern: `entry` names one character or one grouped emoji sequence,
//! `grouping` lazily splits the input into entries, `render` spells an entry's codepoints, name
//! and category within the entry budget, and this module walks the grouped input within the
//! reply budget.

mod entry;
mod grouping;
mod render;
#[cfg(test)]
mod tests;

use zeta_plugin::irc::PLAIN_REPLY_PREFIX;
use zeta_utils::irc::MAX_LISTING_LENGTH;

use self::grouping::GroupedEntries;
use self::render::SUFFIX;

/// The body budget of a `.uni` reply, after the plain reply prefix.
const REPLY_BUDGET: usize = MAX_LISTING_LENGTH - PLAIN_REPLY_PREFIX.len();
/// The budget of one entry once the outer `SUFFIX` is reserved.
const ENTRY_BUDGET: usize = REPLY_BUDGET - SUFFIX.len();

/// Formats the reply to `.uni`.
///
/// `plain_reply()` supplies the teal `> ` marker the reply opens with; the message itself lists
/// one entry per character, each entry naming its codepoint bold inside braces and its general
/// category in parentheses: `d` answers
/// `{\x0f\x02U+0064\x02\x0310}:\x0f LATIN SMALL LETTER D\x0310 (\x0fLowercase Letter\x0310)`.
/// An emoji code point is named by its emoji name, which is the more informative spelling for
/// the button and squared sets; a run of characters that names an emoji sequence of more than
/// one code point — `🏳️‍🌈` — is grouped into one entry that names the sequence as a whole and
/// carries the category of its base character. A listing that does not fit
/// [`MAX_LISTING_LENGTH`] once the plain reply prefix is added stops with [`SUFFIX`], and a
/// grouped sequence's codepoint list is budgeted the same way.
pub fn unicode_details(args: &str) -> String {
    let mut message = String::with_capacity(REPLY_BUDGET);
    let mut entry = String::with_capacity(ENTRY_BUDGET);
    let mut name = String::new();
    let mut truncated = false;

    // Entries are grouped and rendered one at a time: rendering is what costs, so it stops as
    // soon as the next entry cannot fit, and the grouping never buffers the whole input.
    // `SUFFIX` is reserved exactly as `append_entries_with_more` would.
    for (appended, item) in GroupedEntries::new(args).enumerate() {
        item.render_into(&mut entry, &mut name);
        let separator = if appended == 0 { "" } else { ", " };
        if appended > 0
            && message.len() + separator.len() + entry.len() + SUFFIX.len() > REPLY_BUDGET
        {
            truncated = true;
            break;
        }

        message.push_str(separator);
        message.push_str(&entry);
    }

    if truncated {
        message.push_str(SUFFIX);
    }

    message
}
