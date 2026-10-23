//! Emoji entry canonicalization and the emoji section encoding.

use std::collections::{BTreeMap, BTreeSet};

use zeta_unicode_format::{MAX_KEY, format_toned, is_tone, tone_phrase, tone_positions};

use crate::table::push_u16;

/// How many un-reproducible tone variants `ZETA_UNICODE_STATS` prints as examples.
const MAX_EMOJI_SAMPLES: usize = 8;

/// Build-time counters for the emoji section, reported under `ZETA_UNICODE_STATS`.
#[derive(Clone, Debug, Default)]
pub struct EmojiStats {
    /// Stored entries: tone-free bases plus any variants kept verbatim.
    pub entries: usize,
    /// Bases whose tone variants are all reproduced by one of the four formatting rules.
    pub ruled_bases: usize,
    /// Tone variants that no rule reproduces and are therefore stored verbatim.
    pub verbatim_variants: usize,
    /// Tone-variant groups with no stored base to derive from.
    pub missing_bases: usize,
    /// A few `want`/`got` examples of the verbatim variants.
    pub samples: Vec<String>,
    /// The encoded section payload's size, in bytes.
    pub bytes: usize,
}

/// One skin-tone variant: its tone indices, its name and its canonical code points.
type ToneVariant = (Vec<usize>, String, Vec<u32>);

/// Tone variants grouped by the sequence they derive from (with the tones removed).
type ToneVariants = BTreeMap<Vec<u32>, Vec<ToneVariant>>;

/// Builds the emoji section payload from the canonical entries.
pub fn emoji_section(emoji_text: &str, named_text: &str) -> (Vec<u8>, EmojiStats) {
    let mut stats = EmojiStats::default();
    let entries = canonical_emoji_entries(emoji_text, named_text, &mut stats);

    let mut keys = Vec::new();
    let mut key_offsets = Vec::new();
    let mut names = Vec::new();
    let mut name_offsets = Vec::new();
    let mut codes = vec![0u8; (entries.len() * 3).div_ceil(8)];
    let mut tone_masks = vec![0u8; (entries.len() * 2).div_ceil(8)];
    for (index, (key, name, code, tone_mask)) in entries.iter().enumerate() {
        push_len(&mut key_offsets, keys.len(), "key offset");
        push_len(&mut name_offsets, names.len(), "name offset");
        keys.extend_from_slice(key.as_bytes());
        assert!(
            key.len() <= MAX_KEY,
            "emoji key {key:?} is {} bytes, over the decoder's MAX_KEY",
            key.len()
        );
        names.extend_from_slice(name.as_bytes());
        insert_bits(&mut codes, index, 3, *code);
        insert_bits(&mut tone_masks, index, 2, *tone_mask);
    }
    push_len(&mut key_offsets, keys.len(), "key offset");
    push_len(&mut name_offsets, names.len(), "name offset");

    let mut section = Vec::new();
    for (what, length) in [
        ("entries", entries.len()),
        ("key bytes", keys.len()),
        ("name bytes", names.len()),
        ("code bytes", codes.len()),
        ("tone-mask bytes", tone_masks.len()),
    ] {
        push_len(&mut section, length, what);
    }
    section.extend_from_slice(&keys);
    section.extend_from_slice(&key_offsets);
    section.extend_from_slice(&names);
    section.extend_from_slice(&name_offsets);
    section.extend_from_slice(&codes);
    section.extend_from_slice(&tone_masks);
    stats.bytes = section.len();
    (section, stats)
}

/// Appends `length` as a `u16`, naming the field when it does not fit.
fn push_len(buf: &mut Vec<u8>, length: usize, what: &str) {
    push_u16(
        buf,
        u16::try_from(length).unwrap_or_else(|_| panic!("{what} fits in u16")),
    );
}

/// Writes the low `width` bits of `value` at `index * width`, even across a byte boundary.
fn insert_bits(blob: &mut [u8], index: usize, width: usize, value: u8) {
    let bit = index * width;
    let [low, high] = (u16::from(value) << (bit % 8)).to_le_bytes();
    blob[bit / 8] |= low;
    if let Some(slot) = blob.get_mut(bit / 8 + 1) {
        *slot |= high;
    }
}

/// Parses the emoji entries with lookup precedence: named sequences (3) over fully-qualified
/// (2) over minimally-qualified and unqualified (0). Keys keep `U+FE0F` here.
///
/// `NamedSequences.txt` also names non-emoji sequences (Latin letters with combining marks);
/// only the entries whose canonical key names an emoji sequence from `emoji-test.txt` are
/// kept. In practice those are the keycaps, whose official names override the test file's
/// lower-case ones.
fn parse_emoji(emoji_text: &str, named_text: &str) -> BTreeMap<Vec<u32>, (u8, String)> {
    let mut entries: BTreeMap<Vec<u32>, (u8, String)> = BTreeMap::new();

    // The line numbers in the panic messages are the file's, so the loops enumerate every
    // line and skip the comments inside.
    for (number, line) in emoji_text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (left, rest) = line
            .split_once(';')
            .unwrap_or_else(|| panic!("line {}: {line}", number + 1));
        let (status, comment) = rest.split_once('#').expect("comment");
        let cps = parse_code_points(left, number + 1);
        let rank = match status.trim() {
            "fully-qualified" | "component" => 2u8,
            "minimally-qualified" => 1,
            "unqualified" => 0,
            other => panic!("line {}: unknown status {other}", number + 1),
        };
        let words: Vec<&str> = comment.split_whitespace().collect();
        assert!(
            words.len() >= 3,
            "line {}: unexpected comment: {comment}",
            number + 1
        );
        // The comment is `<emoji> <E-version> <name>`; the name may contain anything, `#`
        // included (the `#` keycap).
        keep_best(&mut entries, cps, rank, words[2..].join(" "));
    }

    let emoji_keys: BTreeSet<Vec<u32>> = entries.keys().map(|cps| canonical_cps(cps)).collect();

    for (number, line) in named_text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, cps) = line
            .split_once(';')
            .unwrap_or_else(|| panic!("line {}: {line}", number + 1));
        let cps = parse_code_points(cps, number + 1);
        if !emoji_keys.contains(&canonical_cps(&cps)) {
            continue;
        }
        keep_best(&mut entries, cps, 3, name.trim().to_owned());
    }

    entries
}

/// The canonical key of a sequence: `U+FE0F` removed.
fn canonical_cps(cps: &[u32]) -> Vec<u32> {
    cps.iter().copied().filter(|&cp| cp != 0xFE0F).collect()
}

/// Inserts an entry unless a higher-precedence name already covers the same code points.
fn keep_best(
    entries: &mut BTreeMap<Vec<u32>, (u8, String)>,
    cps: Vec<u32>,
    rank: u8,
    name: String,
) {
    match entries.get(&cps) {
        Some((existing, _)) if *existing > rank => {}
        _ => {
            entries.insert(cps, (rank, name));
        }
    }
}

/// Collects the emoji entries to store: canonicalized keys (U+FE0F removed, so
/// text-presentation variants merge into their emoji-presentation form) with skin-tone
/// variants synthesized from a formatting rule instead of stored. Returns
/// `(key, name, tone rule, tone-count mask)` sorted by key bytes.
fn canonical_emoji_entries(
    emoji_text: &str,
    named_text: &str,
    stats: &mut EmojiStats,
) -> Vec<(String, String, u8, u8)> {
    let all = parse_emoji(emoji_text, named_text);

    // Canonicalize: strip U+FE0F, keeping the highest-precedence name per canonical key.
    let mut canonical: BTreeMap<Vec<u32>, (u8, String)> = BTreeMap::new();
    for (cps, (rank, name)) in &all {
        keep_best(&mut canonical, canonical_cps(cps), *rank, name.clone());
    }

    // Split into bases (no skin-tone code point) and tone variants (dropped, synthesized).
    let mut kept: BTreeMap<Vec<u32>, (String, u8, u8)> = BTreeMap::new();
    let mut variants: ToneVariants = BTreeMap::new();
    for (cps, (_rank, name)) in &canonical {
        let tones = tone_positions(cps.iter().copied());
        if tones.is_empty() || cps.len() == 1 {
            // Standalone entries; a lone skin tone is one of the five components.
            kept.insert(cps.clone(), (name.clone(), 0, 0));
        } else {
            let base: Vec<u32> = cps.iter().copied().filter(|cp| !is_tone(*cp)).collect();
            variants
                .entry(base)
                .or_default()
                .push((tones, name.clone(), cps.clone()));
        }
    }

    // Assign each base the formatting rule that reproduces all of its tone variants; a base
    // whose variants do not agree keeps them stored verbatim, so nothing is lost.
    for (base, group) in &variants {
        // The 2-bit mask can only publish one- and two-tone counts. Reject the whole group
        // before rule selection: a rule that happens to reproduce a three-tone name would
        // otherwise hide a variant the decoder can never reach.
        if group
            .iter()
            .any(|(tones, _, _)| !(1..=2).contains(&tones.len()))
        {
            stats.verbatim_variants += group.len();
            store_verbatim(&mut kept, group);
            continue;
        }

        let (base_name, mut rule) = match kept.get(base) {
            Some((name, _, _)) => (name.clone(), 0),
            // No base entry (a tone-stripped sequence that Unicode does not list on its own):
            // reconstruct one from the variants, and store it so lookups can reach it.
            None if let Some((name, rule)) = derive_base(group) => {
                kept.insert(base.clone(), (name.clone(), rule, 0));
                (name, rule)
            }
            None => {
                stats.missing_bases += 1;
                stats.verbatim_variants += group.len();
                store_verbatim(&mut kept, group);
                continue;
            }
        };

        if rule == 0 {
            // Prefer the deduplicating rules when both reproduce the published variants: the
            // same-tone ZWJ forms (which Unicode represents with a different single codepoint)
            // would otherwise get a duplicated tone phrase.
            for candidate in [3u8, 1, 4, 2] {
                if group.iter().all(|(tones, variant_name, _)| {
                    format_toned(candidate, &base_name, tones) == *variant_name
                }) {
                    rule = candidate;
                    break;
                }
            }
        }

        // Published tone counts are 1 or 2, checked above.
        let tone_mask = group.iter().fold(0u8, |mask, (tones, _, _)| {
            mask | match tones.len() {
                1 => 1,
                2 => 2,
                _ => 0,
            }
        });
        if rule == 0 {
            stats.verbatim_variants += group.len();
            store_verbatim(&mut kept, group);
            continue;
        }

        {
            let entry = kept.get_mut(base).expect("base entry exists");
            entry.1 = rule;
            entry.2 = tone_mask;
        }
        for (tones, variant_name, full_cps) in group {
            let formatted = format_toned(rule, &base_name, tones);
            if formatted != *variant_name {
                stats.verbatim_variants += 1;
                if stats.samples.len() < MAX_EMOJI_SAMPLES {
                    stats.samples.push(format!(
                        "{base_name:?} + {tones:?} -> want {variant_name:?}, got {formatted:?}"
                    ));
                }
                kept.insert(full_cps.clone(), (variant_name.clone(), 0, 0));
            }
        }
        stats.ruled_bases += 1;
    }

    let mut entries: Vec<(String, String, u8, u8)> = kept
        .into_iter()
        .map(|(cps, (name, code, tone_mask))| {
            let key: String = cps
                .iter()
                .map(|&cp| char::from_u32(cp).expect("code point is a scalar value"))
                .collect();
            (key, name, code, tone_mask)
        })
        .collect();
    entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    for pair in entries.windows(2) {
        assert!(pair[0].0 < pair[1].0, "duplicate emoji key {:?}", pair[0].0);
    }
    stats.entries = entries.len();
    entries
}

/// Stores every variant of `group` verbatim, so a group that no rule reproduces is not lost.
fn store_verbatim(kept: &mut BTreeMap<Vec<u32>, (String, u8, u8)>, group: &[ToneVariant]) {
    for (_tones, variant_name, full_cps) in group {
        kept.insert(full_cps.clone(), (variant_name.clone(), 0, 0));
    }
}

/// Reconstructs the base name and rule for a tone-variant group whose tone-stripped sequence is
/// not itself an emoji entry: each candidate rule is inverted on the first variant's name and
/// then has to reproduce the whole group.
fn derive_base(group: &[ToneVariant]) -> Option<(String, u8)> {
    let (tones, first_name, _) = group.first()?;
    [3u8, 1, 4, 2].into_iter().find_map(|rule| {
        let base = strip_phrase(rule, first_name, tones)?;
        group
            .iter()
            .all(|(tones, name, _)| format_toned(rule, &base, tones) == *name)
            .then_some((base, rule))
    })
}

/// The inverse of [`format_toned`] for one variant: removes the tone phrase `rule` would insert.
fn strip_phrase(rule: u8, name: &str, tones: &[usize]) -> Option<String> {
    let phrase = tone_phrase(rule, tones);
    match rule {
        2 | 4 => name
            .strip_suffix(&format!(", {phrase}"))
            .or_else(|| name.strip_suffix(&format!(": {phrase}")))
            .map(str::to_owned),
        _ => {
            // The phrase sits right behind the base's first `": "`.
            let (head, rest) = name.split_once(": ")?;
            let tail = rest.strip_prefix(&format!("{phrase}, "))?;
            Some(format!("{head}: {tail}"))
        }
    }
}

/// Parses a whitespace-separated list of hexadecimal code points.
fn parse_code_points(text: &str, line: usize) -> Vec<u32> {
    let cps: Vec<u32> = text
        .split_whitespace()
        .map(|cp| u32::from_str_radix(cp, 16).expect("code point"))
        .collect();
    assert!(!cps.is_empty(), "line {line}: no code points");
    cps
}

#[cfg(test)]
mod tests {
    use super::*;

    const HANDSHAKE: &str = "\
1FAF1 200D 1FAF2 ; fully-qualified # x E14.0 handshake
1FAF1 1F3FB 200D 1FAF2 1F3FB ; fully-qualified # x E14.0 handshake: light skin tone
1FAF1 1F3FB 200D 1FAF2 1F3FC ; fully-qualified # x E14.0 handshake: light skin tone, medium-light skin tone
";

    fn entries(text: &str) -> Vec<(String, String, u8, u8)> {
        canonical_emoji_entries(text, "", &mut EmojiStats::default())
    }

    fn find<'a>(
        entries: &'a [(String, String, u8, u8)],
        key: &str,
    ) -> &'a (String, String, u8, u8) {
        entries
            .iter()
            .find(|entry| entry.0.as_str() == key)
            .expect("entry")
    }

    #[test]
    fn prefers_the_deduplicating_rule_for_same_tone_groups() {
        let entries = entries(HANDSHAKE);
        let base = find(&entries, "🫱‍🫲");
        assert!(
            matches!(base.2, 3 | 4),
            "a deduplicating rule is required, got {}",
            base.2
        );
        assert_eq!(base.3, 2, "the group only publishes two-tone variants");
    }

    #[test]
    fn derives_a_base_with_the_deduplicating_rule() {
        let without_base = HANDSHAKE
            .lines()
            .filter(|line| !line.starts_with("1FAF1 200D"))
            .collect::<Vec<_>>()
            .join("\n");
        let entries = entries(&without_base);
        let base = find(&entries, "🫱‍🫲");
        assert!(matches!(base.2, 3 | 4), "got rule {}", base.2);
        assert_eq!(base.3, 2);
    }

    #[test]
    fn stores_every_variant_when_no_rule_matches() {
        let no_rule = "\
1F44D ; fully-qualified # x E0.6 thumbs up
1F44D 1F3FB ; fully-qualified # x E0.6 thumbs up: light skin tone
1F44D 1F3FC ; fully-qualified # x E0.6 thumbs up, medium skin tone
";
        let mut stats = EmojiStats::default();
        let entries = canonical_emoji_entries(no_rule, "", &mut stats);
        assert_eq!(entries.len(), 3);
        assert_eq!(stats.verbatim_variants, 2);
        assert_eq!(find(&entries, "👍").2, 0);
        assert_eq!(find(&entries, "👍🏻").2, 0);
        assert_eq!(find(&entries, "👍🏼").2, 0);
    }

    #[test]
    fn stores_every_variant_when_a_group_has_three_tones() {
        // The 2-bit mask cannot publish a three-tone count; storing the variant verbatim keeps
        // it reachable even if one of the formatting rules happens to reproduce its name.
        let three = "\
1F44D ; fully-qualified # x E0.6 thumbs up
1F44D 1F3FB 1F3FC 1F3FD ; fully-qualified # x E0.6 thumbs up: light skin tone, medium-light skin tone, medium skin tone
";
        let mut stats = EmojiStats::default();
        let entries = canonical_emoji_entries(three, "", &mut stats);
        assert_eq!(entries.len(), 2);
        assert_eq!(stats.verbatim_variants, 1);
        assert_eq!(find(&entries, "👍").2, 0);
        assert_eq!(find(&entries, "👍🏻🏼🏽").2, 0);
    }

    #[test]
    fn strip_phrase_inverts_every_supported_rule() {
        for (rule, base) in [
            (1, "handshake: base"),
            (3, "women holding hands: base"),
            (2, "thumbs up"),
            (4, "thumbs up: base"),
        ] {
            let tones = [0usize, 1];
            let formatted = format_toned(rule, base, &tones);
            assert_eq!(
                strip_phrase(rule, &formatted, &tones).as_deref(),
                Some(base),
                "rule {rule}"
            );
        }
    }

    #[test]
    fn records_published_tone_counts() {
        let one_tone = "\
1F44D ; fully-qualified # x E0.6 thumbs up
1F44D 1F3FB ; fully-qualified # x E0.6 thumbs up: light skin tone
";
        let entries = entries(one_tone);
        let base = find(&entries, "👍");
        assert_ne!(base.2, 0, "the one-tone variant has a formatting rule");
        assert_eq!(base.3, 1, "the base only publishes one-tone variants");
    }
}
