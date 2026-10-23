//! Emoji entry canonicalization and the emoji section encoding.

use std::collections::{BTreeMap, BTreeSet};

use crate::table::push_u16;

/// The five emoji skin-tone code points, in tone order.
const TONES: [u32; 5] = [0x1F3FB, 0x1F3FC, 0x1F3FD, 0x1F3FE, 0x1F3FF];

/// The CLDR names of the five skin tones, in the same order; keep in sync with the decoder.
const TONE_NAMES: [&str; 5] = [
    "light skin tone",
    "medium-light skin tone",
    "medium skin tone",
    "medium-dark skin tone",
    "dark skin tone",
];

/// Longest canonical emoji key, in bytes, that the decoder's `MAX_KEY` scratch can hold.
///
/// Keep in sync with `MAX_KEY` in `src/lib.rs`: a longer key would silently become an
/// emoji miss there instead of a build failure here.
const MAX_KEY: usize = 64;

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
    for (index, (key, name, code)) in entries.iter().enumerate() {
        push_u16(
            &mut key_offsets,
            u16::try_from(keys.len()).expect("keys fit in u16"),
        );
        push_u16(
            &mut name_offsets,
            u16::try_from(names.len()).expect("names fit in u16"),
        );
        keys.extend_from_slice(key.as_bytes());
        assert!(
            key.len() <= MAX_KEY,
            "emoji key {key:?} is {} bytes, over the decoder's MAX_KEY",
            key.len()
        );
        names.extend_from_slice(name.as_bytes());
        let bit = index * 3;
        let byte = bit / 8;
        let shift = bit % 8;
        // The 3-bit code can straddle a byte boundary, so it is OR-ed into two bytes — the
        // decoder reads it the same way.
        let [low, high] = (u16::from(*code) << shift).to_le_bytes();
        codes[byte] |= low;
        if let Some(slot) = codes.get_mut(byte + 1) {
            *slot |= high;
        }
    }
    push_u16(
        &mut key_offsets,
        u16::try_from(keys.len()).expect("keys fit in u16"),
    );
    push_u16(
        &mut name_offsets,
        u16::try_from(names.len()).expect("names fit in u16"),
    );

    let mut section = Vec::new();
    push_u16(
        &mut section,
        u16::try_from(entries.len()).expect("entries fit in u16"),
    );
    push_u16(
        &mut section,
        u16::try_from(keys.len()).expect("keys fit in u16"),
    );
    push_u16(
        &mut section,
        u16::try_from(names.len()).expect("names fit in u16"),
    );
    push_u16(
        &mut section,
        u16::try_from(codes.len()).expect("codes fit in u16"),
    );
    section.extend_from_slice(&keys);
    section.extend_from_slice(&key_offsets);
    section.extend_from_slice(&names);
    section.extend_from_slice(&name_offsets);
    section.extend_from_slice(&codes);
    stats.bytes = section.len();
    (section, stats)
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

    for (number, line) in emoji_text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .enumerate()
    {
        let (left, rest) = line
            .split_once(';')
            .unwrap_or_else(|| panic!("line {number}: {line}"));
        let (status, comment) = rest.split_once('#').expect("comment");
        let cps = parse_code_points(left, number);
        let rank = match status.trim() {
            "fully-qualified" | "component" => 2u8,
            "minimally-qualified" => 1,
            "unqualified" => 0,
            other => panic!("line {number}: unknown status {other}"),
        };
        let words: Vec<&str> = comment.split_whitespace().collect();
        assert!(
            words.len() >= 3,
            "line {number}: unexpected comment: {comment}"
        );
        // The comment is `<emoji> <E-version> <name>`; the name may contain anything, `#`
        // included (the `#` keycap).
        keep_best(&mut entries, cps, rank, words[2..].join(" "));
    }

    let emoji_keys: BTreeSet<Vec<u32>> = entries.keys().map(|cps| canonical_cps(cps)).collect();

    for (number, line) in named_text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .enumerate()
    {
        let (name, cps) = line
            .split_once(';')
            .unwrap_or_else(|| panic!("line {number}: {line}"));
        let cps = parse_code_points(cps, number);
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
/// `(key, name, tone rule)` sorted by key bytes.
fn canonical_emoji_entries(
    emoji_text: &str,
    named_text: &str,
    stats: &mut EmojiStats,
) -> Vec<(String, String, u8)> {
    let all = parse_emoji(emoji_text, named_text);

    // Canonicalize: strip U+FE0F, keeping the highest-precedence name per canonical key.
    let mut canonical: BTreeMap<Vec<u32>, (u8, String)> = BTreeMap::new();
    for (cps, (rank, name)) in &all {
        let key = canonical_cps(cps);
        match canonical.get(&key) {
            Some((existing, _)) if *existing > *rank => {}
            _ => {
                canonical.insert(key, (*rank, name.clone()));
            }
        }
    }

    // Split into bases (no skin-tone code point) and tone variants (dropped, synthesized).
    let mut kept: BTreeMap<Vec<u32>, (String, u8)> = BTreeMap::new();
    let mut variants: ToneVariants = BTreeMap::new();
    for (cps, (_rank, name)) in &canonical {
        let tones = tone_positions(cps);
        if tones.is_empty() || cps.len() == 1 {
            // Standalone entries; a lone skin tone is one of the five components.
            kept.insert(cps.clone(), (name.clone(), 0));
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
        let (base_name, mut rule) = match kept.get(base) {
            Some((name, _)) => (name.clone(), 0),
            // No base entry (a tone-stripped sequence that Unicode does not list on its own):
            // reconstruct one from the variants, and store it so lookups can reach it.
            None if let Some((name, rule)) = derive_base(group) => {
                kept.insert(base.clone(), (name.clone(), rule));
                (name, rule)
            }
            None => {
                stats.missing_bases += 1;
                for (_tones, variant_name, full_cps) in group {
                    stats.verbatim_variants += 1;
                    kept.insert(full_cps.clone(), (variant_name.clone(), 0));
                }
                continue;
            }
        };

        if rule == 0 {
            for candidate in 1..=4u8 {
                if group.iter().all(|(tones, variant_name, _)| {
                    format_toned(candidate, &base_name, tones) == *variant_name
                }) {
                    rule = candidate;
                    break;
                }
            }
        }
        kept.get_mut(base).expect("base entry exists").1 = rule;
        for (tones, variant_name, full_cps) in group {
            let formatted = format_toned(rule, &base_name, tones);
            if formatted != *variant_name {
                stats.verbatim_variants += 1;
                if stats.samples.len() < MAX_EMOJI_SAMPLES {
                    stats.samples.push(format!(
                        "{base_name:?} + {tones:?} -> want {variant_name:?}, got {formatted:?}"
                    ));
                }
                kept.insert(full_cps.clone(), (variant_name.clone(), 0));
            }
        }
        if rule != 0 {
            stats.ruled_bases += 1;
        }
    }

    let mut entries: Vec<(String, String, u8)> = kept
        .into_iter()
        .map(|(cps, (name, code))| {
            let key: String = cps
                .iter()
                .map(|&cp| char::from_u32(cp).expect("code point is a scalar value"))
                .collect();
            (key, name, code)
        })
        .collect();
    entries.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
    for pair in entries.windows(2) {
        assert!(pair[0].0 < pair[1].0, "duplicate emoji key {:?}", pair[0].0);
    }
    stats.entries = entries.len();
    entries
}

/// Reconstructs the base name and rule for a tone-variant group whose tone-stripped sequence is
/// not itself an emoji entry: each candidate rule is inverted on the first variant's name and
/// then has to reproduce the whole group.
fn derive_base(group: &[ToneVariant]) -> Option<(String, u8)> {
    let (tones, first_name, _) = group.first()?;
    (1..=4u8).find_map(|rule| {
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

/// Indices (into [`TONES`]) of the skin-tone code points in `cps`, in order.
fn tone_positions(cps: &[u32]) -> Vec<usize> {
    cps.iter()
        .filter_map(|cp| TONES.iter().position(|&tone| tone == *cp))
        .collect()
}

fn is_tone(cp: u32) -> bool {
    TONES.contains(&cp)
}

/// Renders `base` with `tones` applied, following the 3-bit rule:
/// `1` inserts the tone phrase after the first `": "`, `2` appends it, `3`/`4` do the same
/// after collapsing consecutive duplicate tones.
fn format_toned(code: u8, base: &str, tones: &[usize]) -> String {
    let phrase = tone_phrase(code, tones);

    match (base.find(": "), code) {
        (Some(colon), 1 | 3) => {
            format!("{}: {}, {}", &base[..colon], phrase, &base[colon + 2..])
        }
        (Some(_), _) => format!("{base}, {phrase}"),
        (None, _) => format!("{base}: {phrase}"),
    }
}

/// The comma-joined tone names, collapsing consecutive duplicates for rules 3 and 4.
fn tone_phrase(code: u8, tones: &[usize]) -> String {
    let mut tone_list = tones.to_vec();
    if code == 3 || code == 4 {
        tone_list.dedup();
    }
    tone_list
        .iter()
        .map(|&index| TONE_NAMES[index])
        .collect::<Vec<_>>()
        .join(", ")
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
