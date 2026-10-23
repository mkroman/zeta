//! The `.uni` renderer: names, general categories and grouped emoji sequences.
//!
//! The renderer keeps each entry cheap until the byte budget decides whether to render it,
//! because one padded emoji sequence can exhaust a whole IRC line on its own.

use std::borrow::Cow;

use zeta_unicode::{
    MAX_EMOJI_KEY_BYTES, emoji_name, general_category, may_start_emoji_sequence, name_or_label,
};
use zeta_utils::irc::{MAX_LISTING_LENGTH, append_entries_within_budget};

use crate::prelude::*;

/// The complete prefix `notice()` writes, budgeted out of the listings.
const NOTICE_PREFIX_LEN: usize = NOTICE_PREFIX.len();
/// The body budget of a `.uni` reply, after the notice prefix.
const REPLY_BUDGET: usize = MAX_LISTING_LENGTH - NOTICE_PREFIX_LEN;
/// The budget of one entry once the outer `SUFFIX` is reserved.
const ENTRY_BUDGET: usize = REPLY_BUDGET - SUFFIX.len();
/// Appended when a listing does not fit the budget.
const SUFFIX: &str = ", …";
/// Longest run of characters `.uni` probes for an emoji sequence.
///
/// A stored emoji key is at most [`MAX_EMOJI_KEY_BYTES`] bytes, hence at most that many
/// characters. Only stripped `U+FE0F` padding can make a matching input run longer, so the cap
/// bounds the probe work: a longer padded run is split into a matched prefix and the remaining
/// characters rather than hiding the rest of the input.
const MAX_GROUPING_CHARS: usize = MAX_EMOJI_KEY_BYTES;

/// One entry of a `.uni` listing, kept cheap until the budget decides to render it.
enum Entry<'a> {
    /// A single character, named by its character or emoji name.
    Char(char),
    /// A run of two or more characters naming an emoji sequence as a whole.
    Sequence {
        /// The input text that matched, used for the codepoint list.
        text: &'a str,
        /// The published sequence name, before upper-casing.
        name: Cow<'static, str>,
    },
}

impl Entry<'_> {
    /// Renders the entry in the `.uni` wire format.
    fn render(&self) -> String {
        match self {
            Self::Char(c) => char_entry(*c),
            Self::Sequence { text, name } => sequence_entry(text, &uppercase(name)),
        }
    }
}

/// Formats the reply to `.uni`.
///
/// `notice()` supplies the teal `> ` marker the reply opens with; the message itself lists one
/// entry per character, each entry naming its codepoint bold inside braces and its general
/// category in parentheses: `d` answers
/// `{\x0f\x02U+0064\x02\x0310}:\x0f LATIN SMALL LETTER D\x0310 (\x0fLowercase Letter\x0310)`.
/// An emoji code point is named by its emoji name, which is the more informative spelling for
/// the button and squared sets; a run of characters that names an emoji sequence of more than
/// one code point — `🏳️‍🌈` — is grouped into one entry that names the sequence as a whole and
/// carries the category of its base character. A listing that does not fit
/// [`MAX_LISTING_LENGTH`] once the notice prefix is added stops with [`SUFFIX`], and a grouped
/// sequence's codepoint list is budgeted the same way.
pub(super) fn unicode_details(args: &str) -> String {
    let entries = grouped_entries(args);
    let total = entries.len();
    let mut message = String::with_capacity(REPLY_BUDGET);
    let appended = append_entries_within_budget(
        &mut message,
        entries.iter().map(Entry::render),
        ", ",
        REPLY_BUDGET,
        SUFFIX.len(),
    );

    if appended < total {
        message.push_str(SUFFIX);
    }

    message
}

/// Splits `args` into character entries, grouping each run that names an emoji sequence of at
/// least two code points into a single entry.
fn grouped_entries(args: &str) -> Vec<Entry<'_>> {
    let bounds: Vec<usize> = args
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(args.len()))
        .collect();
    let last = bounds.len() - 1;
    let mut entries = Vec::new();
    let mut at = 0usize;

    while at < last {
        // The longest run from here that names a sequence; a single code point is not a
        // grouping but a character entry of its own. Plain text cannot start a stored key, so
        // reject it before probing every candidate end.
        let end_limit = last.min(at.saturating_add(MAX_GROUPING_CHARS));
        let grouped = if may_start_emoji_sequence(&args[bounds[at]..]) {
            (at + 2..=end_limit)
                .rev()
                .find_map(|end| Some((end, emoji_name(&args[bounds[at]..bounds[end]])?)))
        } else {
            None
        };

        if let Some((end, name)) = grouped {
            entries.push(Entry::Sequence {
                text: &args[bounds[at]..bounds[end]],
                name,
            });
            at = end;
        } else {
            let c = args[bounds[at]..]
                .chars()
                .next()
                .expect("a boundary starts a character");
            entries.push(Entry::Char(c));
            at += 1;
        }
    }

    entries
}

/// Renders an entry: its codepoints bold inside the braces, then name and category.
///
/// The codepoint list is a listing too: canonicalization strips every `U+FE0F`, so the input
/// that matched can be much longer than the stored key, and the list stops with [`SUFFIX`]
/// inside the braces. `tail` is the exact text that follows the list, so the reserved budget
/// cannot drift from what is written.
fn render_entry(points: &str, name: &str, category: &str) -> String {
    let tail = format!("{BOLD}{COLOR}}}:{RESET} {name}{COLOR} ({RESET}{category}{COLOR})");
    let mut entry = format!("{{{RESET}{BOLD}");

    // The first codepoint is appended unconditionally, so the entry only fits while the tail
    // leaves room for the prefix and `U+10FFFF` on top of the reserved bytes.
    debug_assert!(
        entry.len() + "U+10FFFF".len() + SUFFIX.len() + tail.len() <= ENTRY_BUDGET,
        "the name {name:?} does not leave room for the codepoint list"
    );

    let total = points.chars().count();
    let appended = append_entries_within_budget(
        &mut entry,
        points.chars().map(|c| format!("U+{:04X}", u32::from(c))),
        ", ",
        ENTRY_BUDGET,
        SUFFIX.len() + tail.len(),
    );
    if appended < total {
        entry.push_str(SUFFIX);
    }
    entry.push_str(&tail);

    entry
}

/// Renders one grouped emoji sequence with the category of its base character.
fn sequence_entry(text: &str, name: &str) -> String {
    let base = text
        .chars()
        .next()
        .expect("a sequence has a base character");
    let category = general_category(base).long_name();
    render_entry(text, name, category)
}

/// Renders one character entry.
///
/// An emoji code point is named by its emoji name — the CLDR spelling says what a character is
/// for (`AB button (blood type)`) where the character name spells its glyph (`NEGATIVE SQUARED
/// AB`) — falling back to [`name_or_label`] for everything else.
fn char_entry(c: char) -> String {
    let mut encoded = [0u8; 4];
    let text = c.encode_utf8(&mut encoded);
    let name: Cow<'static, str> =
        emoji_name(text).map_or_else(|| name_or_label(c), |name| Cow::Owned(name.to_uppercase()));
    let category = general_category(c).long_name();
    render_entry(text, &name, category)
}

/// Upper-cases `name`, borrowing it unchanged when it has no lower-case characters.
fn uppercase(name: &str) -> Cow<'_, str> {
    if name.chars().any(char::is_lowercase) {
        Cow::Owned(name.to_uppercase())
    } else {
        Cow::Borrowed(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uni_names_a_single_character() {
        assert_eq!(
            unicode_details("d"),
            "{\x0f\x02U+0064\x02\x0310}:\x0f LATIN SMALL LETTER D\x0310 \
             (\x0fLowercase Letter\x0310)"
        );
        assert_eq!(
            unicode_details("ˇ"),
            "{\x0f\x02U+02C7\x02\x0310}:\x0f CARON\x0310 \
             (\x0fModifier Letter\x0310)"
        );
    }

    #[test]
    fn uni_names_an_emoji_sequence_as_a_whole() {
        assert_eq!(
            unicode_details("🏳️‍🌈"),
            "{\x0f\x02U+1F3F3, U+FE0F, U+200D, U+1F308\x02\x0310}:\x0f RAINBOW FLAG\x0310 \
             (\x0fOther Symbol\x0310)"
        );
        assert_eq!(
            unicode_details("2️⃣"),
            "{\x0f\x02U+0032, U+FE0F, U+20E3\x02\x0310}:\x0f KEYCAP DIGIT TWO\x0310 \
             (\x0fDecimal Number\x0310)"
        );
        // A single code point is a character entry of its own.
        assert_eq!(
            unicode_details("🐗"),
            "{\x0f\x02U+1F417\x02\x0310}:\x0f BOAR\x0310 (\x0fOther Symbol\x0310)"
        );
    }

    #[test]
    fn uni_names_an_emoji_code_point_by_its_emoji_name() {
        // `NEGATIVE SQUARED AB` is the character name; the emoji name says what it is for.
        assert_eq!(
            unicode_details("🆎"),
            "{\x0f\x02U+1F18E\x02\x0310}:\x0f AB BUTTON (BLOOD TYPE)\x0310 \
             (\x0fOther Symbol\x0310)"
        );
    }

    #[test]
    fn uni_groups_a_sequence_beside_a_character() {
        assert_eq!(
            unicode_details("🏳️‍🌈🦔"),
            "{\x0f\x02U+1F3F3, U+FE0F, U+200D, U+1F308\x02\x0310}:\x0f RAINBOW FLAG\x0310 \
             (\x0fOther Symbol\x0310), \
             {\x0f\x02U+1F994\x02\x0310}:\x0f HEDGEHOG\x0310 (\x0fOther Symbol\x0310)"
        );
    }

    #[test]
    fn uni_uses_the_published_tone_count() {
        let same_tone = unicode_details("👩🏻‍🤝‍👩🏻");
        assert!(same_tone.contains("WOMEN HOLDING HANDS: LIGHT SKIN TONE"));
        assert!(!same_tone.contains("LIGHT SKIN TONE, LIGHT SKIN TONE"));

        // A second tone on a one-tone base is not a sequence; the valid prefix is grouped and
        // the trailing modifier is listed as its own character.
        let invalid = unicode_details("👍🏻🏿");
        assert!(invalid.contains("THUMBS UP: LIGHT SKIN TONE"));
        assert!(invalid.contains("DARK SKIN TONE"));
    }

    #[test]
    fn uni_lists_single_emoji_as_characters() {
        assert_eq!(
            unicode_details("🦔🫠"),
            "{\x0f\x02U+1F994\x02\x0310}:\x0f HEDGEHOG\x0310 (\x0fOther Symbol\x0310), \
             {\x0f\x02U+1FAE0\x02\x0310}:\x0f MELTING FACE\x0310 (\x0fOther Symbol\x0310)"
        );
    }

    #[test]
    fn uni_lists_every_characters_name_and_category() {
        assert_eq!(
            unicode_details("d'êtr"),
            "{\x0f\x02U+0064\x02\x0310}:\x0f LATIN SMALL LETTER D\x0310 \
             (\x0fLowercase Letter\x0310), \
             {\x0f\x02U+0027\x02\x0310}:\x0f APOSTROPHE\x0310 (\x0fOther Punctuation\x0310), \
             {\x0f\x02U+00EA\x02\x0310}:\x0f LATIN SMALL LETTER E WITH CIRCUMFLEX\x0310 \
             (\x0fLowercase Letter\x0310), \
             {\x0f\x02U+0074\x02\x0310}:\x0f LATIN SMALL LETTER T\x0310 \
             (\x0fLowercase Letter\x0310), \
             {\x0f\x02U+0072\x02\x0310}:\x0f LATIN SMALL LETTER R\x0310 \
             (\x0fLowercase Letter\x0310)"
        );
    }

    #[test]
    fn uni_names_control_characters_and_combining_marks() {
        let reply = unicode_details("\u{0}");
        assert!(reply.contains("NULL"), "{reply}");
        assert!(reply.contains("Control"), "{reply}");

        let reply = unicode_details("e\u{301}");
        assert!(reply.contains("LATIN SMALL LETTER E"), "{reply}");
        assert!(reply.contains("COMBINING ACUTE ACCENT"), "{reply}");
        assert_eq!(grouped_entries("e\u{301}").len(), 2);
    }

    #[test]
    fn uni_with_an_empty_argument_is_empty() {
        assert_eq!(unicode_details(""), "");
        assert!(grouped_entries("").is_empty());
    }

    #[test]
    fn notice_prefix_matches_the_budget() {
        assert_eq!(notice("").len(), NOTICE_PREFIX_LEN);
        assert_eq!(NOTICE_PREFIX_LEN, "\x0310> ".len());
    }

    #[test]
    fn grouped_entries_keeps_a_sequence_whole() {
        let entries = grouped_entries("a🏳️‍🌈");
        assert!(matches!(entries[0], Entry::Char('a')));
        match &entries[1] {
            Entry::Sequence { text, .. } => assert_eq!(*text, "🏳️‍🌈"),
            Entry::Char(_) => panic!("the rainbow flag must be grouped"),
        }
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn grouped_entries_skips_plain_text_without_probing() {
        let entries = grouped_entries("abc");
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().all(|entry| matches!(entry, Entry::Char(_))));
    }

    #[test]
    fn uni_groups_a_regional_indicator_flag() {
        let reply = unicode_details("🇩🇰");
        assert!(reply.contains("U+1F1E9, U+1F1F0"), "{reply}");
        assert!(reply.contains("FLAG: DENMARK"), "{reply}");
    }

    #[test]
    fn uni_stops_within_the_listing_budget() {
        assert!(!unicode_details("é").contains('…'));

        let repeated = "é".repeat(60);
        let reply = unicode_details(&repeated);
        assert!(reply.contains('…'), "reply was not truncated: {reply}");
        assert!(reply.ends_with(", …"));
        assert!(
            reply.len() + NOTICE_PREFIX_LEN <= MAX_LISTING_LENGTH,
            "{}",
            reply.len() + NOTICE_PREFIX_LEN
        );
    }

    #[test]
    fn uni_budgets_a_padded_emoji_sequence() {
        // `U+FE0F` is stripped by canonicalization, so this still matches the rainbow
        // flag; without a budget the points list would be much longer than the input.
        let padded = format!("🏳️‍🌈{}", "\u{FE0F}".repeat(60));
        let reply = unicode_details(&padded);

        assert!(reply.contains('…'), "reply was not truncated: {reply}");
        assert!(
            reply.len() + NOTICE_PREFIX_LEN <= MAX_LISTING_LENGTH,
            "{}",
            reply.len() + NOTICE_PREFIX_LEN
        );
        assert!(reply.ends_with("Other Symbol\x0310)"), "{reply}");
    }

    #[test]
    fn uni_splits_a_sequence_longer_than_the_grouping_cap() {
        let padded = format!("🏳️‍🌈{}", "\u{FE0F}".repeat(62));
        let entries = grouped_entries(&padded);

        match &entries[0] {
            Entry::Sequence { text, .. } => {
                assert_eq!(text.chars().count(), MAX_GROUPING_CHARS);
            }
            Entry::Char(_) => panic!("the rainbow flag prefix must be grouped"),
        }
        assert!(
            entries[1..]
                .iter()
                .all(|entry| matches!(entry, Entry::Char('\u{FE0F}')))
        );
    }

    /// Times the two `.uni` shapes that matter: plain text and emoji-heavy input.
    ///
    /// ```sh
    /// cargo test -p plugins --all-features --release -- --ignored --nocapture string_utils_benchmark
    /// ```
    #[test]
    #[ignore = "benchmark"]
    fn string_utils_benchmark() {
        use std::time::Instant;

        for (label, input) in [
            ("ascii", "a".repeat(MAX_LISTING_LENGTH)),
            ("emoji", "🏳️‍🌈".repeat(30)),
        ] {
            let rounds = 200u32;
            let start = Instant::now();
            let mut sink = 0usize;
            for _ in 0..rounds {
                let reply = unicode_details(std::hint::black_box(input.as_str()));
                sink += std::hint::black_box(reply).len();
            }
            eprintln!(
                "{label:>6}: {:>8?}/reply (sink {sink})",
                start.elapsed() / rounds
            );
        }
    }

    #[test]
    fn uni_reserves_the_outer_suffix_for_a_grouped_sequence() {
        // The sequence entry must leave room for the outer `, …` when a following character
        // does not fit.
        let input = format!("\u{2198}{}a", "\u{FE0F}".repeat(43));
        let reply = unicode_details(&input);
        assert!(reply.ends_with(", …"), "{reply}");
        assert!(!reply.contains("U+0061"), "{reply}");
        assert!(
            reply.len() + NOTICE_PREFIX_LEN <= MAX_LISTING_LENGTH,
            "{}",
            reply.len() + NOTICE_PREFIX_LEN
        );
    }
}
