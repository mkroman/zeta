//! Generates the packed Unicode table from the vendored UCD files in `data/`.
//!
//! The build script parses Unicode 18.0.0's `DerivedName`, `DerivedGeneralCategory`,
//! `NameAliases`, `NamedSequences` and `emoji-test` files, derives everything that Unicode
//! defines algorithmically (Hangul syllables, the `*` name patterns), folds character name
//! aliases into the name table, and writes one binary file into `OUT_DIR`:
//!
//! ```text
//! magic "ZUCD" | u16 major, minor, patch | u16 section_count
//! section_count × (u32 id, u32 file_offset, u32 length)
//! section payloads (offsets inside a section are relative to that section)
//! ```
//!
//! The names are emitted by the vendored encoder in `build/build_names.rs`: front-coded against
//! each code point predecessor and split into three PPMd8-I streams, with the names that
//! `data/derived-names.rules` can rebuild left as generated Rust instead of table bytes. The
//! encoder round-trips every name against `DerivedName.txt` while it builds.
//!
//! The general category table is the measured two-level design: a page directory (`u16`
//! first-record index per 2,048-code-point page) over `u16` records packing the
//! page-relative range start (11 bits) and the category id (5 bits, 1..=30 in
//! [`GC_SHORT_NAMES`] order).
//!
//! Emoji keys are canonicalized: `U+FE0F` is stripped, so fully-qualified and text-presentation
//! inputs hit the same key, and every skin-tone variant is *synthesized* at lookup from its
//! base name plus a 3-bit formatting rule — 1,944 stored entries answer for all 5,244.

// Vendored name encoder: front-coded streams, the rule file, the generated tree and the
// round-trip check. It carries more API than `main` calls (the mining sweep lives beside it).
#[allow(
    dead_code,
    reason = "the vendored encoder exposes its sweep helpers next to the emitter"
)]
#[path = "build/build_names.rs"]
mod build_names;

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::PathBuf;

/// Magic at the start of the generated table.
const MAGIC: &[u8; 4] = b"ZUCD";
/// Length of the fixed header: magic, three version components and a section count.
const HEADER_LEN: usize = 4 + 2 * 3 + 2;
/// Length of one section directory entry: id, file offset, payload length.
const ENTRY_LEN: usize = 12;

const SECTION_NAME_RUNS: u32 = 1;
const SECTION_NAME_PATTERNS: u32 = 2;
const SECTION_GC: u32 = 3;
const SECTION_EMOJI: u32 = 4;
const SECTION_NAMES: u32 = 5;

/// The general category abbreviations in table id order (id = position + 1).
///
/// Keep in sync with the decoder's `GC_SHORT_NAMES` in `src/lib.rs`: the crate's round-trip
/// tests walk every range of `DerivedGeneralCategory.txt` and would fail if the two drifted.
const GC_SHORT_NAMES: [&str; 30] = [
    "Cc", "Cf", "Cn", "Co", "Cs", "Ll", "Lm", "Lo", "Lt", "Lu", "Mc", "Me", "Mn", "Nd", "Nl", "No",
    "Pc", "Pd", "Pe", "Pf", "Pi", "Po", "Ps", "Sc", "Sk", "Sm", "So", "Zl", "Zp", "Zs",
];

/// `cp >> GC_SHIFT` selects the page of the general category table.
const GC_SHIFT: u32 = 11;

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

/// The general category range table: `(start, end, table id)`.
type GcRanges = Vec<(u32, u32, u8)>;

/// One skin-tone variant: its tone indices, its name and its canonical code points.
type ToneVariant = (Vec<usize>, String, Vec<u32>);

/// Tone variants grouped by the sequence they derive from (with the tones removed).
type ToneVariants = BTreeMap<Vec<u32>, Vec<ToneVariant>>;

/// How many un-reproducible tone variants `ZETA_UNICODE_STATS` prints as examples.
const MAX_EMOJI_SAMPLES: usize = 8;

/// Longest canonical emoji key, in bytes, that the decoder's `MAX_KEY` scratch can hold.
///
/// Keep in sync with `MAX_KEY` in `src/lib.rs`: a longer key would silently become an
/// emoji miss there instead of a build failure here.
const MAX_KEY: usize = 64;

/// Build-time counters for the emoji section, reported under `ZETA_UNICODE_STATS`.
#[derive(Default)]
struct EmojiStats {
    /// Stored entries: tone-free bases plus any variants kept verbatim.
    entries: usize,
    /// Bases whose tone variants are all reproduced by one of the four formatting rules.
    ruled_bases: usize,
    /// Tone variants that no rule reproduces and are therefore stored verbatim.
    verbatim_variants: usize,
    /// Tone-variant groups with no stored base to derive from.
    missing_bases: usize,
    /// A few `want`/`got` examples of the verbatim variants.
    samples: Vec<String>,
}

/// An emoji entry: canonical code point sequence, name, tone formatting rule.
struct Section {
    id: u32,
    data: Vec<u8>,
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=ZETA_UNICODE_STATS");
    for file in [
        "DerivedName.txt",
        "DerivedGeneralCategory.txt",
        "NameAliases.txt",
        "NamedSequences.txt",
        "emoji-test.txt",
        "derived-names.rules",
    ] {
        println!("cargo:rerun-if-changed=data/{file}");
    }

    let name_file = read_data("DerivedName.txt");
    let gc_file = read_data("DerivedGeneralCategory.txt");
    let alias_file = read_data("NameAliases.txt");
    let sequence_file = read_data("NamedSequences.txt");
    let emoji_file = read_data("emoji-test.txt");

    // Every vendored file must advertise the same version, or the tables would silently mix
    // releases.
    let version = header_version("DerivedName", &name_file);
    for (stem, text) in [
        ("DerivedGeneralCategory", &gc_file),
        ("NameAliases", &alias_file),
        ("NamedSequences", &sequence_file),
    ] {
        assert_eq!(
            header_version(stem, text),
            version,
            "{stem}.txt is not Unicode {version:?}"
        );
    }
    assert_eq!(
        emoji_test_version(&emoji_file),
        (version.0, version.1),
        "emoji-test.txt is not Unicode {version:?}"
    );

    let gc = parse_general_category(&gc_file);
    let (emoji, emoji_stats) = emoji_section(&emoji_file, &sequence_file);

    let sections = build_sections(&gc, &emoji);
    let table = pack(sections, version);

    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"))
        .join("zeta_unicode_tables.bin");
    std::fs::write(&out, &table)
        .unwrap_or_else(|e| panic!("failed to write {}: {e}", out.display()));

    if env::var_os("ZETA_UNICODE_STATS").is_some() {
        println!(
            "cargo:warning=zeta-unicode: {} gc ranges -> {} table bytes",
            gc.len(),
            table.len()
        );
        println!(
            "cargo:warning=zeta-unicode: emoji: {} bytes, {} entries ({} ruled bases, {} verbatim variants, {} groups with no base)",
            emoji.len(),
            emoji_stats.entries,
            emoji_stats.ruled_bases,
            emoji_stats.verbatim_variants,
            emoji_stats.missing_bases
        );
        for sample in &emoji_stats.samples {
            println!("cargo:warning=zeta-unicode: emoji verbatim: {sample}");
        }
    }
}

fn read_data(file: &str) -> String {
    let path = PathBuf::from("data").join(file);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
}

/// Reads the `# <Stem>-<major>.<minor>.<patch>.txt` header shared by the UCD files.
fn header_version(stem: &str, text: &str) -> (u8, u8, u8) {
    for line in text.lines().take(8) {
        let Some(rest) = line.strip_prefix("# ") else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(stem).and_then(|r| r.strip_prefix('-')) else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(".txt") else {
            continue;
        };
        let mut parts = rest.split('.');
        let parse = |component: Option<&str>| -> u8 {
            component
                .expect("version component")
                .parse()
                .expect("numeric version component")
        };
        let version = (
            parse(parts.next()),
            parse(parts.next()),
            parse(parts.next()),
        );
        assert!(
            parts.next().is_none(),
            "malformed version header: {stem}-{rest}"
        );
        return version;
    }
    panic!("no version header found for {stem}");
}

/// Reads the `# Version: <major>.<minor>` header of `emoji-test.txt`.
fn emoji_test_version(text: &str) -> (u8, u8) {
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("# Version:") else {
            continue;
        };
        let mut parts = rest.trim().split('.');
        let major = parts
            .next()
            .expect("major version")
            .parse()
            .expect("numeric major version");
        let minor = parts
            .next()
            .expect("minor version")
            .parse()
            .expect("numeric minor version");
        return (major, minor);
    }
    panic!("no version header found for emoji-test");
}

/// The table id (1..=30) of a general category abbreviation.
fn general_category_id(letter: &str) -> u8 {
    let index = GC_SHORT_NAMES
        .iter()
        .position(|short| *short == letter)
        .unwrap_or_else(|| panic!("unknown general category: {letter}"));
    u8::try_from(index).expect("fewer than 255 categories") + 1
}

/// Parses `DerivedGeneralCategory.txt` into sorted, disjoint ranges carrying table ids.
fn parse_general_category(text: &str) -> GcRanges {
    let mut ranges: Vec<(u32, u32, u8)> = Vec::new();

    for line in text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let (lhs, rest) = line.split_once(';').expect("field separator");
        let letter = rest.split('#').next().expect("category field").trim();
        assert_eq!(letter.len(), 2, "unexpected general category: {letter}");

        let lhs = lhs.trim();
        let (start, end) = match lhs.split_once("..") {
            Some((start, end)) => (start, end),
            None => (lhs, lhs),
        };
        let start = u32::from_str_radix(start, 16).expect("range start");
        let end = u32::from_str_radix(end, 16).expect("range end");
        assert!(start <= end, "reversed range U+{start:04X}..U+{end:04X}");

        let id = general_category_id(letter);
        ranges.push((start, end, id));
    }

    ranges.sort_by_key(|(start, _, _)| *start);
    for pair in ranges.windows(2) {
        assert!(
            pair[0].1 < pair[1].0,
            "overlapping general category ranges U+{:04X}..U+{:04X} and U+{:04X}..U+{:04X}",
            pair[0].0,
            pair[0].1,
            pair[1].0,
            pair[1].1
        );
    }

    let mut seen = [false; 31];
    for &(_, _, id) in &ranges {
        seen[usize::from(id)] = true;
    }
    for (index, present) in seen.iter().enumerate().skip(1) {
        assert!(
            present,
            "general category {} is unused",
            GC_SHORT_NAMES[index - 1]
        );
    }

    ranges
}

/// Builds the general category section: the page directory followed by the packed records.
fn general_category_section(ranges: &GcRanges) -> Vec<u8> {
    let mut records = Vec::with_capacity(ranges.len() * 2);
    for (start, _, id) in ranges {
        let page = start >> GC_SHIFT;
        let relative = start - (page << GC_SHIFT);
        let record = u16::try_from((relative << 5) | u32::from(*id))
            .expect("page-relative start and id fit in 16 bits");
        assert_eq!(
            u32::from(record >> 5),
            relative,
            "page-relative start is lossy"
        );
        records.extend_from_slice(&record.to_le_bytes());
    }

    let pages = (0x0011_0000 >> GC_SHIFT) + 1;
    let mut directory = Vec::with_capacity(pages * 2);
    let mut index = 0usize;
    for page in 0..pages {
        let base = u32::try_from(page << GC_SHIFT).expect("page base fits in u32");
        while index < ranges.len() && ranges[index].0 < base {
            index += 1;
        }
        push_u16(
            &mut directory,
            u16::try_from(index).expect("fewer than 65536 ranges"),
        );
    }

    directory.extend_from_slice(&records);
    directory
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

/// Builds the emoji section payload from the canonical entries.
fn emoji_section(emoji_text: &str, named_text: &str) -> (Vec<u8>, EmojiStats) {
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
    (section, stats)
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

fn push_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn build_sections(gc: &GcRanges, emoji: &[u8]) -> Vec<Section> {
    // The names are emitted by the vendored encoder: front-coded streams plus the rules that
    // rebuild 6,677 of the names from their code point instead of storing them.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let ledger = build_names::emit(&build_names::Request {
        // The tree is included by the crate, so it has to land under the name the crate expects.
        gen_rs: Some(out_dir.join("zeta_unicode_derive.rs")),
        ..build_names::Request::new("data", "data/derived-names.rules", &out_dir)
    });

    if env::var_os("ZETA_UNICODE_STATS").is_some() {
        println!(
            "cargo:warning=zeta-unicode: names: {} entries ({} derived, {} stored), \
             {} arms over {} atoms ({} B), streams {}/{}/{}, names {} + runs {} + patterns {} \
             = {} B ({} B charging the atoms)",
            ledger.entries,
            ledger.derived,
            ledger.stored,
            ledger.arms,
            ledger.atoms,
            ledger.atom_bytes,
            ledger.streams[0].compressed,
            ledger.streams[1].compressed,
            ledger.streams[2].compressed,
            ledger.blob,
            ledger.runs.compressed,
            ledger.pattern_section.len(),
            ledger.table,
            ledger.table_strict
        );
    }

    let read = |name: &str| -> Vec<u8> {
        std::fs::read(out_dir.join(name)).unwrap_or_else(|e| panic!("failed to read {name}: {e}"))
    };

    vec![
        Section {
            id: SECTION_NAME_RUNS,
            data: read("runs.bin"),
        },
        Section {
            id: SECTION_NAME_PATTERNS,
            data: ledger.pattern_section,
        },
        Section {
            id: SECTION_GC,
            data: general_category_section(gc),
        },
        Section {
            id: SECTION_EMOJI,
            data: emoji.to_vec(),
        },
        Section {
            id: SECTION_NAMES,
            data: read("names.bin"),
        },
    ]
}

/// Writes the header, the section directory and the payloads.
fn pack(sections: Vec<Section>, version: (u8, u8, u8)) -> Vec<u8> {
    let directory_len = HEADER_LEN + sections.len() * ENTRY_LEN;
    let mut table = Vec::with_capacity(directory_len);
    table.extend_from_slice(MAGIC);
    push_u16(&mut table, u16::from(version.0));
    push_u16(&mut table, u16::from(version.1));
    push_u16(&mut table, u16::from(version.2));
    push_u16(
        &mut table,
        u16::try_from(sections.len()).expect("fewer than 65536 sections"),
    );

    let mut offset = u32::try_from(directory_len).expect("directory fits in u32");
    for section in &sections {
        let length = u32::try_from(section.data.len()).expect("section fits in u32");
        push_u32(&mut table, section.id);
        push_u32(&mut table, offset);
        push_u32(&mut table, length);
        offset = offset.checked_add(length).expect("offsets fit in u32");
    }
    for section in sections {
        table.extend_from_slice(&section.data);
    }
    table
}
