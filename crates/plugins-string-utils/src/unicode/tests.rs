//! Tests for the `.uni` renderer: exact control codes, grouping and byte budgets.

use zeta_plugin::irc::{BOLD, RESET, plain_reply};
use zeta_unicode::general_category;
use zeta_utils::irc::MAX_LISTING_LENGTH;

use super::entry::{Entry, character_name_into};
use super::grouping::{MAX_GROUPING_CHARS, grouped_entries};
use super::render::{ENTRY_OPEN, SUFFIX, TAIL_FIXED_LEN, WIDEST_POINT, entry_tail, render_entry};
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
