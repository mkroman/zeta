//! The `.uni` renderer: names, general categories and grouped emoji sequences.
//!
//! The renderer keeps each entry cheap until the byte budget decides whether to render it,
//! because one padded emoji sequence can exhaust a whole IRC line on its own.

use std::borrow::Cow;
use std::fmt::Write as _;
use std::iter::Peekable;
use std::str::CharIndices;

use zeta_unicode::{
    MAX_EMOJI_KEY_BYTES, emoji_name, general_category, may_start_emoji_sequence, name_or_label,
    write_name,
};
use zeta_utils::irc::{BOLD, COLOR, MAX_LISTING_LENGTH, RESET};

use zeta_plugin::prelude::*;

/// The body budget of a `.uni` reply, after the plain reply prefix.
const REPLY_BUDGET: usize = MAX_LISTING_LENGTH - PLAIN_REPLY_PREFIX.len();
/// The budget of one entry once the outer `SUFFIX` is reserved.
const ENTRY_BUDGET: usize = REPLY_BUDGET - SUFFIX.len();
/// Appended when a listing does not fit the budget.
const SUFFIX: &str = ", …";
/// The bytes that open an entry's codepoint list: `{` followed by the reset and bold codes.
const ENTRY_OPEN: &str = "{\x0f\x02";
/// The widest a code point can be spelled, used to prove an entry always fits.
const WIDEST_POINT: &str = "U+10FFFF";
/// The bytes of an entry's tail around the name and category.
const TAIL_FIXED_LEN: usize = BOLD.len()
    + COLOR.len()
    + "}:".len()
    + RESET.len()
    + " ".len()
    + COLOR.len()
    + " (".len()
    + RESET.len()
    + COLOR.len()
    + ")".len();
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
    /// Renders the entry into `out`, reusing `name` for the entry's upper-cased name.
    ///
    /// `out` is cleared first, so a listing renders every entry through the same buffers.
    fn render_into(&self, out: &mut String, name: &mut String) {
        match self {
            Self::Char(c) => {
                character_name_into(*c, name);
                let category = general_category(*c).long_name();
                let mut encoded = [0u8; 4];
                render_entry_into(c.encode_utf8(&mut encoded), name, category, out);
            }
            Self::Sequence {
                text,
                name: sequence_name,
            } => {
                write_uppercase(name, sequence_name);
                let base = text
                    .chars()
                    .next()
                    .expect("a sequence has a base character");
                let category = general_category(base).long_name();
                render_entry_into(text, name, category, out);
            }
        }
    }
}

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

/// The character entries of an input, grouped lazily as [`grouped_entries`] describes.
struct GroupedEntries<'a> {
    /// The input text every entry borrows from.
    args: &'a str,
    /// The characters that have not been grouped yet.
    chars: Peekable<CharIndices<'a>>,
}

impl<'a> GroupedEntries<'a> {
    /// Starts grouping `args` from its first character.
    fn new(args: &'a str) -> Self {
        Self {
            args,
            chars: args.char_indices().peekable(),
        }
    }
}

impl<'a> Iterator for GroupedEntries<'a> {
    type Item = Entry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let (start, c) = self.chars.next()?;
        // Plain text cannot start a stored key, so the admission check rejects it before
        // probing every candidate end.
        let Some((end, name)) = longest_sequence(self.args, start, &self.chars) else {
            return Some(Entry::Char(c));
        };

        while self.chars.peek().is_some_and(|&(offset, _)| offset < end) {
            self.chars.next();
        }

        Some(Entry::Sequence {
            text: &self.args[start..end],
            name,
        })
    }
}

/// Splits `args` into character entries, grouping each run that names an emoji sequence of at
/// least two code points into a single entry.
#[cfg(test)]
fn grouped_entries(args: &str) -> Vec<Entry<'_>> {
    GroupedEntries::new(args).collect()
}

/// The longest run starting at `start` that names an emoji sequence, with the byte offset just
/// past it.
///
/// A run is at most [`MAX_GROUPING_CHARS`] characters long, so the probe reads a bounded window
/// into a fixed-size stack buffer instead of materializing the whole input.
fn longest_sequence(
    args: &str,
    start: usize,
    rest: &Peekable<CharIndices<'_>>,
) -> Option<(usize, Cow<'static, str>)> {
    if !may_start_emoji_sequence(&args[start..]) {
        return None;
    }

    // `ends[length]` is the byte offset just past a run of `length` characters; the slot after
    // the last filled one is the end of the longest run the cap allows.
    let mut ends = [args.len(); MAX_GROUPING_CHARS + 1];
    let mut lookahead = rest.clone();
    let mut length = 1usize;

    while length < MAX_GROUPING_CHARS {
        let Some((offset, _)) = lookahead.next() else {
            break;
        };
        ends[length] = offset;
        length += 1;
    }
    ends[length] = lookahead.peek().map_or(args.len(), |&(offset, _)| offset);

    (2..=length)
        .rev()
        .find_map(|end| emoji_name(&args[start..ends[end]]).map(|name| (ends[end], name)))
}

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
fn entry_tail(name: &str, category: &str) -> String {
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
fn render_entry_into(points: &str, name: &str, category: &str, entry: &mut String) {
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
fn render_entry(points: &str, name: &str, category: &str) -> String {
    let mut entry = String::with_capacity(ENTRY_BUDGET);
    render_entry_into(points, name, category, &mut entry);
    entry
}

/// Writes the upper-cased name a character entry shows into `out`.
fn character_name_into(c: char, out: &mut String) {
    out.clear();
    let mut encoded = [0u8; 4];
    let text = c.encode_utf8(&mut encoded);
    if let Some(name) = emoji_name(text) {
        write_uppercase(out, name.as_ref());
    } else if !write_name(c, out) {
        out.push_str(&name_or_label(c));
    }
}

/// Upper-cases `name` into `out`, borrowing the buffer rather than allocating a string.
fn write_uppercase(out: &mut String, name: &str) {
    out.clear();
    for c in name.chars() {
        if c.is_lowercase() {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
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
    fn the_entry_opening_is_the_expected_control_code_sequence() {
        assert_eq!(ENTRY_OPEN, format!("{{{RESET}{BOLD}"));
        assert_eq!(plain_reply(""), PLAIN_REPLY_PREFIX);
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
            plain_reply(&reply).len() <= MAX_LISTING_LENGTH,
            "{}",
            plain_reply(&reply).len()
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
            plain_reply(&reply).len() <= MAX_LISTING_LENGTH,
            "{}",
            plain_reply(&reply).len()
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
            plain_reply(&reply).len() <= MAX_LISTING_LENGTH,
            "{}",
            plain_reply(&reply).len()
        );
    }

    #[test]
    fn the_entry_tail_fixed_length_matches_the_rendered_tail() {
        assert_eq!(entry_tail("", "").len(), TAIL_FIXED_LEN);
    }

    #[test]
    fn every_char_entry_tail_fits_the_entry_budget() {
        let tail_base_len = entry_tail("", "").len();
        let mut longest: Option<(usize, char, String, &'static str)> = None;

        for cp in 0..=0x0010_FFFF {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            let mut name = String::new();
            character_name_into(c, &mut name);
            let category = general_category(c).long_name();
            let tail_len = tail_base_len + name.len() + category.len();

            if longest
                .as_ref()
                .is_none_or(|(longest_len, ..)| tail_len > *longest_len)
            {
                longest = Some((tail_len, c, name, category));
            }
        }

        let (max_tail_len, c, name, category) = longest.expect("the scalar range has characters");
        assert_eq!(entry_tail(&name, category).len(), max_tail_len);
        assert!(
            ENTRY_OPEN.len() + WIDEST_POINT.len() + SUFFIX.len() + max_tail_len <= ENTRY_BUDGET,
            "U+{:04X} ({name:?}, {category}) appends a {max_tail_len}-byte tail",
            u32::from(c)
        );

        // The worst character as a single-point entry also fits.
        let entry = render_entry(&c.to_string(), &name, category);
        assert!(entry.len() <= ENTRY_BUDGET, "{entry:?}");
    }

    #[test]
    fn render_entry_budgets_the_longest_codepoint_list() {
        let (name, category) = ("X", "Other Symbol");
        let tail = entry_tail(name, category);
        // `WIDEST_POINT` is the widest a code point can be spelled.
        let point = char::MAX;
        let point_len = format!("U+{:04X}", u32::from(point)).len();
        let separator_len = ", ".len();
        let full_len = |points: usize| {
            ENTRY_OPEN.len() + points * point_len + (points - 1) * separator_len + tail.len()
        };

        // The list is appended while the entry plus the reserved outer suffix fits; one more
        // point would need the suffix instead.
        let max_complete = (1..=ENTRY_BUDGET)
            .take_while(|&points| full_len(points) + SUFFIX.len() <= ENTRY_BUDGET)
            .last()
            .expect("one point always fits");
        let points = |count: usize| point.to_string().repeat(count);

        let complete = render_entry(&points(max_complete), name, category);
        assert!(!complete.contains(SUFFIX), "{complete:?}");
        assert_eq!(complete.len(), full_len(max_complete));
        assert!(complete.len() <= ENTRY_BUDGET, "{}", complete.len());

        let truncated = render_entry(&points(max_complete + 1), name, category);
        assert!(
            truncated.ends_with(&format!("{SUFFIX}{tail}")),
            "{truncated:?}"
        );
        assert_eq!(truncated.len(), complete.len() + SUFFIX.len());
        assert!(truncated.len() <= ENTRY_BUDGET, "{}", truncated.len());

        // A far longer list is cut at the same point.
        let long = render_entry(&points(60), name, category);
        assert_eq!(long, truncated);
        assert!(long.len() <= ENTRY_BUDGET, "{}", long.len());
    }
}
