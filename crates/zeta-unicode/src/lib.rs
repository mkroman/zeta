//! Unicode 18.0.0 character names, general categories and emoji sequence names.
//!
//! The `zeta-unicode-build` crate compiles the vendored UCD files in `data/` into one packed
//! table that is linked into the binary with `include_bytes!`. A lookup walks that table
//! directly: there is no per-lookup parsing or decompression, and stored names are borrowed.
//! The name streams are decoded once, on the first name lookup. The general category table is a
//! two-level page structure (a `u16` directory plus `u16` range records), and emoji keys are
//! canonicalized — `U+FE0F` is stripped and skin-tone variants are synthesized from a 3-bit
//! rule plus a 2-bit tone-count mask — so 1,944 stored keys answer for every one of the 3,972
//! sequences the UCD names.
//!
//! ```
//! use zeta_unicode::{emoji_name, general_category, name, name_or_label};
//!
//! assert_eq!(name('ß').unwrap(), "LATIN SMALL LETTER SHARP S");
//! assert_eq!(general_category('ß').long_name(), "Lowercase Letter");
//! assert_eq!(emoji_name("🏳️‍🌈").unwrap(), "rainbow flag");
//! assert_eq!(name_or_label('\u{f8ff}'), "<private-use-F8FF>");
//! ```

mod emoji;
mod gc;
mod names;
mod sections;
mod tables;

use std::borrow::Cow;

use zeta_unicode_format::{GC_PAGES, GC_SHIFT, HANGUL_END, HANGUL_START, MAX_KEY, UNICODE_END};

/// Returns the CLDR short name of an emoji sequence such as `🏳️‍🌈`.
///
/// Names come from `emoji-test.txt`, or from `NamedSequences.txt` for official names. Keys are
/// canonicalized (`U+FE0F` is optional) and skin-tone variants are synthesized from their base,
/// so the name is returned as published rather than cased for a reply. See [`emoji::emoji_name`]
/// for the full contract.
pub use emoji::emoji_name;
pub use gc::GeneralCategory;

/// The Unicode version of the data this crate was built from, as `(major, minor, patch)`.
///
/// `zeta-unicode-build` refuses to build when the vendored files disagree, and the table header
/// is checked at compile time, so this cannot drift from the table.
pub const UNICODE_VERSION: (u8, u8, u8) = (18, 0, 0);

/// The target size budget for the generated table, in bytes.
///
/// The current reference encoder does not meet it yet; until the compressed encoder lands the
/// test suite holds the table to the encoder's own `ENCODER_TABLE_LIMIT` instead.
pub const TABLE_BUDGET: usize = 100_000;

/// The longest emoji key the table stores, in bytes.
///
/// `.uni` uses it to bound how far it probes for a multi-character emoji sequence.
pub const MAX_EMOJI_KEY_BYTES: usize = MAX_KEY;

/// The upper bound the current reference encoder is held to, well above [`TABLE_BUDGET`]: the
/// compressed encoder that meets the budget has not landed yet.
#[cfg(test)]
const ENCODER_TABLE_LIMIT: usize = 130_000;

/// Choongbae (leading) jamo short names, in `Jamo_Short_Name` order.
const JAMO_LEADING: [&str; 19] = [
    "G", "GG", "N", "D", "DD", "R", "M", "B", "BB", "S", "SS", "", "J", "JJ", "C", "K", "T", "P",
    "H",
];
/// Vowel (medial) jamo short names.
const JAMO_VOWEL: [&str; 21] = [
    "A", "AE", "YA", "YAE", "EO", "E", "YEO", "YE", "O", "WA", "WAE", "OE", "YO", "U", "WEO", "WE",
    "WI", "YU", "EU", "YI", "I",
];
/// Trailing jamo short names, index 0 being "no trailing consonant".
const JAMO_TRAILING: [&str; 28] = [
    "", "G", "GG", "GS", "N", "NJ", "NH", "D", "L", "LG", "LM", "LB", "LS", "LT", "LP", "LH", "M",
    "B", "BS", "S", "SS", "NG", "J", "C", "K", "T", "P", "H",
];

/// Returns the `Name` property of `c`, a formal alias from `NameAliases.txt` when the `Name`
/// property is empty (the control code points), or the derived name Unicode defines
/// algorithmically.
///
/// Stored names come back borrowed; names built from a pattern (`CJK UNIFIED IDEOGRAPH-4E00`)
/// or from the Hangul jamo tables are allocated.
#[must_use]
pub fn name(c: char) -> Option<Cow<'static, str>> {
    if let Some(stored) = names::name_for(u32::from(c)) {
        return Some(Cow::Borrowed(stored));
    }
    let mut buf = String::new();
    derived_name(c, &mut buf).then_some(Cow::Owned(buf))
}

/// Appends the name of `c` to `buf`, returning `false` when `c` has no name.
///
/// Stored names are copied straight out of the table and Hangul or pattern names are written
/// into `buf` without a temporary allocation.
///
/// # Examples
///
/// ```
/// let mut buf = String::new();
/// assert!(zeta_unicode::write_name('中', &mut buf));
/// assert_eq!(buf, "CJK UNIFIED IDEOGRAPH-4E2D");
/// ```
#[must_use = "the return value reports whether `c` has a name"]
pub fn write_name(c: char, buf: &mut String) -> bool {
    if let Some(stored) = names::name_for(u32::from(c)) {
        buf.push_str(stored);
        return true;
    }
    derived_name(c, buf)
}

/// Returns the name of `c`, or the label Unicode defines for code points without one.
///
/// Labels are `<private-use-F8FF>`, `<reserved-0378>`, `<noncharacter-FFFE>` and
/// `<control-0000>`; they are rendered on the spot because storing them would cost more than
/// the six bytes they take. Controls with a formal alias answer with the alias instead (the
/// current data gives every `Cc` code point one), so the control label is the fallback.
#[must_use]
pub fn name_or_label(c: char) -> Cow<'static, str> {
    if let Some(name) = name(c) {
        return name;
    }

    let cp = u32::from(c);
    let kind = match general_category(c) {
        GeneralCategory::PrivateUse => "private-use",
        GeneralCategory::Control => "control",
        GeneralCategory::Surrogate => "surrogate",
        GeneralCategory::Unassigned if is_noncharacter(cp) => "noncharacter",
        _ => "reserved",
    };
    Cow::Owned(format!("<{kind}-{cp:04X}>"))
}

/// Returns the general category of `c`; unassigned points answer with
/// [`GeneralCategory::Unassigned`].
#[must_use]
pub fn general_category(c: char) -> GeneralCategory {
    general_category_of(u32::from(c)).unwrap_or(GeneralCategory::Unassigned)
}

/// Is `cp` one of Unicode's noncharacters (`U+FDD0..U+FDEF` or `*FFFE`/`*FFFF`)?
///
/// Values above `U+10FFFF` are not code points and answer `false`.
#[must_use]
pub const fn is_noncharacter(cp: u32) -> bool {
    cp <= UNICODE_END && (matches!(cp, 0xFDD0..=0xFDEF) || matches!(cp & 0xFFFF, 0xFFFE | 0xFFFF))
}

/// Writes `cp` as upper-case hex digits into `out`, returning how many bytes it wrote.
///
/// The shared hole filler for `*` name patterns and for the generated rule tree, so both fill a
/// hole the same way. `width` is a *minimum*, the way Unicode writes the placeholder:
/// `EGYPTIAN HIEROGLYPH-13460` needs five digits where most code points need four.
fn write_hex(out: &mut [u8; 8], cp: u32, width: u8) -> usize {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut needed = 0usize;
    let mut rest = cp;
    while rest > 0 {
        rest >>= 4;
        needed += 1;
    }
    // Unicode always writes at least one digit, even for zero.
    let count = needed.max(usize::from(width.min(8))).max(1);
    let mut shift = u32::try_from(count - 1).expect("at most eight digits") * 4;
    for slot in out.iter_mut().take(count) {
        *slot = DIGITS[((cp >> shift) & 0xF) as usize];
        shift = shift.saturating_sub(4);
    }
    count
}

/// Appends `cp` as upper-case hex digits to `buf`.
fn push_hex(buf: &mut String, cp: u32, width: u8) {
    let mut digits = [0u8; 8];
    let used = write_hex(&mut digits, cp, width);
    buf.push_str(std::str::from_utf8(&digits[..used]).expect("hex digits are ASCII"));
}

/// Formats a derived name (an `*` pattern or a Hangul syllable) into `buf`.
///
/// Returns `false` when `c` has no name at all.
fn derived_name(c: char, buf: &mut String) -> bool {
    let cp = u32::from(c);

    if (HANGUL_START..=HANGUL_END).contains(&cp) {
        let index = cp - HANGUL_START;
        let leading = JAMO_LEADING[(index / 588) as usize];
        let vowel = JAMO_VOWEL[((index % 588) / 28) as usize];
        let trailing = JAMO_TRAILING[(index % 28) as usize];
        buf.push_str("HANGUL SYLLABLE ");
        buf.push_str(leading);
        buf.push_str(vowel);
        buf.push_str(trailing);
        return true;
    }

    pattern_name(cp, buf)
}

/// Writes the `*` pattern covering `cp` into `buf`, prefix and suffix included.
///
/// Records are `(start, end)` followed by the two halves of Unicode's `*` pattern, so the name
/// is `prefix + hex(cp) + suffix`. They are stored plainly: there are twenty of them and a name
/// miss reads them on every call.
fn pattern_name(cp: u32, buf: &mut String) -> bool {
    let Some(records) = tables::section(sections::NAME_PATTERNS) else {
        return false;
    };
    let mut at = 0usize;
    while at < records.len() {
        let (Some(start), Some(end), Some(prefix_len)) = (
            tables::u32_at(records, at),
            tables::u32_at(records, at + 4),
            tables::u16_at(records, at + 8),
        ) else {
            return false;
        };
        let prefix_at = at + 10;
        let Some(prefix) = tables::bytes_at(records, prefix_at, usize::from(prefix_len)) else {
            return false;
        };
        let suffix_len_at = prefix_at + usize::from(prefix_len);
        let Some(suffix_len) = tables::u16_at(records, suffix_len_at) else {
            return false;
        };
        let suffix_at = suffix_len_at + 2;
        let Some(suffix) = tables::bytes_at(records, suffix_at, usize::from(suffix_len)) else {
            return false;
        };
        at = suffix_at + usize::from(suffix_len);

        if cp < start {
            return false;
        }
        if cp <= end {
            let (Ok(prefix), Ok(suffix)) =
                (std::str::from_utf8(prefix), std::str::from_utf8(suffix))
            else {
                return false;
            };
            buf.push_str(prefix);
            push_hex(buf, cp, 0);
            buf.push_str(suffix);
            return true;
        }
    }
    false
}

/// Looks up the general category of `cp` in the two-level page table.
fn general_category_of(cp: u32) -> Option<GeneralCategory> {
    let table = tables::section(sections::GC)?;
    let directory = tables::bytes_at(table, 0, GC_PAGES * 2)?;
    let records = table.get(GC_PAGES * 2..).unwrap_or_default();

    let page = usize::try_from(cp >> GC_SHIFT).ok()?;
    let relative = cp & ((1 << GC_SHIFT) - 1);
    let first = usize::from(tables::u16_at(directory, page.checked_mul(2)?)?);
    let end = usize::from(tables::u16_at(
        directory,
        page.checked_mul(2)?.checked_add(2)?,
    )?);

    // Find the last run in the page whose start is at or before `cp`. When no run starts in
    // this page (`first == end`) or `cp` falls before the page's first run, `low` stays at
    // `first` and the carried-over run ending just before it answers instead.
    let (mut low, mut high) = (first, end);
    while low < high {
        let mid = low + (high - low) / 2;
        let start = u32::from(tables::u16_at(records, mid.checked_mul(2)?)? >> 5);
        if start <= relative {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    let record = low.checked_sub(1)?;

    let id = usize::from(tables::u16_at(records, record.checked_mul(2)?)? & 31);
    GeneralCategory::from_id(id)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    const DERIVED_NAME: &str = include_str!("../data/DerivedName.txt");
    const DERIVED_GC: &str = include_str!("../data/DerivedGeneralCategory.txt");
    const NAME_ALIASES: &str = include_str!("../data/NameAliases.txt");
    const EMOJI_TEST: &str = include_str!("../data/emoji-test.txt");
    const NAMED_SEQUENCES: &str = include_str!("../data/NamedSequences.txt");

    #[test]
    fn unicode_version_matches_the_vendored_data() {
        let header = DERIVED_NAME.lines().next().expect("header line");
        let version = header
            .trim_start_matches("# DerivedName-")
            .trim_end_matches(".txt");
        let parsed: Vec<u8> = version
            .split('.')
            .map(|part| part.parse().expect("numeric version component"))
            .collect();
        assert_eq!(
            (parsed[0], parsed[1], parsed[2]),
            UNICODE_VERSION,
            "UNICODE_VERSION is stale"
        );
        assert_eq!(
            tables::version(),
            UNICODE_VERSION,
            "the generated table was built from a different Unicode release"
        );
    }

    #[test]
    fn every_name_in_the_ucd_round_trips() {
        let mut checked = 0usize;

        for line in DERIVED_NAME
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let (lhs, rhs) = line.split_once(';').expect("field separator");
            let lhs = lhs.trim();

            // `DerivedName.txt` spells a pattern as a `*` placeholder "in place of the code
            // point in hex", on a single-code-point line as well as on a range: U+18CFF is the
            // one that matters, since it is the only point whose line carries a `*`.
            let value = rhs.trim();
            let pattern = value.split_once('*');
            if let Some((prefix, _width)) = pattern {
                let range = lhs
                    .split_once("..")
                    .map_or_else(|| (lhs, lhs), |(start, end)| (start, end));
                let start = u32::from_str_radix(range.0, 16).expect("range start");
                let end = u32::from_str_radix(range.1, 16).expect("range end");
                for cp in start..=end {
                    let Some(c) = char::from_u32(cp) else {
                        continue;
                    };
                    assert_eq!(
                        name(c).as_deref(),
                        Some(format!("{prefix}{cp:04X}").as_str()),
                        "pattern name for U+{cp:04X}"
                    );
                    checked += 1;
                }
                continue;
            }

            let cp = u32::from_str_radix(lhs, 16).expect("code point");
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            assert_eq!(name(c).as_deref(), Some(rhs.trim()), "name for U+{cp:04X}");
            checked += 1;
        }

        assert!(checked > 150_000, "only checked {checked} names");
    }

    #[test]
    fn alias_backed_names_are_available() {
        let mut checked = 0usize;

        for line in NAME_ALIASES
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let cp = u32::from_str_radix(line.split(';').next().expect("code point"), 16)
                .expect("code point");
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            assert!(
                name(c).is_some(),
                "U+{cp:04X} has an alias but answered with no name"
            );
            checked += 1;
        }

        assert!(checked > 300, "only checked {checked} aliases");
        assert_eq!(name('\u{16}').as_deref(), Some("SYNCHRONOUS IDLE"));
        assert_eq!(name('\u{9}').as_deref(), Some("CHARACTER TABULATION"));
    }

    #[test]
    fn general_categories_match_the_ucd() {
        let mut checked = 0usize;

        for line in DERIVED_GC
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let (lhs, rest) = line.split_once(';').expect("field separator");
            let letter = rest.split('#').next().expect("category").trim();
            let lhs = lhs.trim();
            let (start, end) = match lhs.split_once("..") {
                Some((start, end)) => (start, end),
                None => (lhs, lhs),
            };
            let start = u32::from_str_radix(start, 16).expect("range start");
            let end = u32::from_str_radix(end, 16).expect("range end");

            for cp in start..=end {
                let Some(c) = char::from_u32(cp) else {
                    continue;
                };
                assert_eq!(
                    general_category(c).short_name(),
                    letter,
                    "general category of U+{cp:04X}"
                );
                checked += 1;
            }
        }

        // Surrogates are not `char` values; everything else must be covered.
        assert!(checked > 1_100_000, "only checked {checked} code points");
        assert_eq!(general_category('A'), GeneralCategory::UppercaseLetter);
        assert_eq!(general_category('あ'), GeneralCategory::OtherLetter);
        assert_eq!(general_category('\u{378}'), GeneralCategory::Unassigned);
    }

    /// Rebuilds the emoji oracle the way `zeta-unicode-build` does: canonical keys (U+FE0F removed) with
    /// named-sequence names taking precedence over fully-qualified, then qualified over
    /// unqualified.
    fn emoji_oracle() -> BTreeMap<String, String> {
        let mut ranked: BTreeMap<Vec<u32>, (u8, String)> = BTreeMap::new();

        for line in EMOJI_TEST
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let (left, rest) = line.split_once(';').expect("field separator");
            let (status, comment) = rest.split_once('#').expect("comment");
            let cps: Vec<u32> = left
                .split_whitespace()
                .map(|cp| u32::from_str_radix(cp, 16).expect("code point"))
                .collect();
            let rank = match status.trim() {
                "fully-qualified" | "component" => 2,
                "minimally-qualified" => 1,
                "unqualified" => 0,
                other => panic!("unknown status {other}"),
            };
            let words: Vec<&str> = comment.split_whitespace().collect();
            match ranked.get(&cps) {
                Some((existing, _)) if *existing > rank => {}
                _ => {
                    ranked.insert(cps, (rank, words[2..].join(" ")));
                }
            }
        }

        // `NamedSequences.txt` also carries non-emoji sequences (Latin letters with combining
        // marks); the build drops those, so the oracle must too.
        let emoji_keys: BTreeSet<Vec<u32>> = ranked
            .keys()
            .map(|cps| cps.iter().copied().filter(|&cp| cp != 0xFE0F).collect())
            .collect();

        for line in NAMED_SEQUENCES
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let (name, cps) = line.split_once(';').expect("field separator");
            let cps: Vec<u32> = cps
                .split_whitespace()
                .map(|cp| u32::from_str_radix(cp, 16).expect("code point"))
                .collect();
            let canonical: Vec<u32> = cps.iter().copied().filter(|&cp| cp != 0xFE0F).collect();
            if !emoji_keys.contains(&canonical) {
                continue;
            }
            ranked.insert(cps, (3, name.trim().to_owned()));
        }

        let mut oracle = BTreeMap::new();
        for (cps, (_, name)) in ranked {
            let key: String = cps
                .iter()
                .filter_map(|&cp| char::from_u32(cp).filter(|c| u32::from(*c) != 0xFE0F))
                .collect();
            oracle.insert(key, name);
        }
        oracle
    }

    #[test]
    fn emoji_sequences_resolve_to_their_names() {
        let oracle = emoji_oracle();

        for (key, name) in &oracle {
            assert_eq!(
                emoji_name(key).as_deref(),
                Some(name.as_str()),
                "emoji name for {key:?}"
            );
        }

        assert!(
            // 5,244 emoji-test entries collapse onto 3,972 canonical keys (U+FE0F merged
            // away); the keycap named sequences only supply names for existing keys.
            oracle.len() > 3_900,
            "only checked {} sequences",
            oracle.len()
        );
        assert_eq!(emoji_name("🏳️‍🌈").as_deref(), Some("rainbow flag"));
        assert_eq!(emoji_name("2️⃣").as_deref(), Some("KEYCAP DIGIT TWO"));
        // Text presentation answers exactly like emoji presentation.
        assert_eq!(emoji_name("🚶‍♂").as_deref(), Some("man walking"));
        // A skin-tone variant is synthesized from its base and the 3-bit rule.
        assert_eq!(
            emoji_name("🫶🏻").as_deref(),
            Some("heart hands: light skin tone")
        );
        assert_eq!(
            emoji_name("🖐🏻").as_deref(),
            Some("hand with fingers splayed: light skin tone")
        );
        assert_eq!(emoji_name("d'être"), None);
        assert_eq!(emoji_name("🐱‍🏍"), None);
        // A non-emoji named sequence (a decomposed Latin letter) is not an emoji.
        assert_eq!(emoji_name("\u{0100}\u{0300}"), None);
    }

    #[test]
    fn tone_fallback_respects_published_tone_counts() {
        // Two tones on a base that only publishes one-tone variants.
        assert_eq!(emoji_name("👍🏻🏿"), None);
        // One tone on a base that only publishes two-tone variants.
        assert_eq!(emoji_name("🫱🏻‍🫲"), None);
        // Published counts still resolve, and same-tone groups deduplicate the phrase.
        assert_eq!(
            emoji_name("👩🏻‍🤝‍👩🏻").as_deref(),
            Some("women holding hands: light skin tone")
        );
        assert_eq!(
            emoji_name("🫱🏻‍🫲🏻").as_deref(),
            Some("handshake: light skin tone")
        );
        assert_eq!(
            emoji_name("🧑🏻‍🤝‍🧑🏻").as_deref(),
            Some("people holding hands: light skin tone")
        );
    }

    #[test]
    fn leading_tone_or_variation_selector_is_not_a_sequence() {
        assert_eq!(emoji_name("🏻👋"), None);
        assert_eq!(emoji_name("\u{FE0F}👍"), None);
        // A lone skin-tone component is still an entry.
        assert_eq!(emoji_name("🏻").as_deref(), Some("light skin tone"));
    }

    #[test]
    fn unnamed_code_points_get_a_label() {
        assert_eq!(name_or_label('\u{f8ff}'), "<private-use-F8FF>");
        assert_eq!(name_or_label('\u{378}'), "<reserved-0378>");
        assert_eq!(name_or_label('\u{FFFE}'), "<noncharacter-FFFE>");
        assert_eq!(name('\u{0}').as_deref(), Some("NULL"));
        // Controls answer with their formal alias rather than the `<control-…>` label.
        assert_eq!(name_or_label('\u{0}'), "NULL");
        assert_eq!(name_or_label('\u{80}'), "PADDING CHARACTER");
    }

    #[test]
    fn name_returns_borrowed_stored_names() {
        assert!(matches!(name('A'), Some(Cow::Borrowed(_))));
        assert!(matches!(name('中'), Some(Cow::Owned(_))));
        assert!(matches!(emoji_name("🦔"), Some(Cow::Borrowed(_))));
        assert!(matches!(emoji_name("🫶🏻"), Some(Cow::Owned(_))));
    }

    #[test]
    fn noncharacter_boundaries_are_exact() {
        for cp in [0xFDD0, 0xFDEF, 0xFFFE, 0xFFFF, 0x0001_FFFE, 0x0010_FFFF] {
            assert!(is_noncharacter(cp), "U+{cp:04X}");
        }
        for cp in [0xFDCF, 0xFDF0, 0x0001_FDD0, 0x0011_FFFE, 0x0011_0000] {
            assert!(!is_noncharacter(cp), "U+{cp:04X}");
        }
    }

    #[test]
    fn write_name_matches_name() {
        let mut buf = String::from("x");
        assert!(write_name('中', &mut buf));
        assert_eq!(buf, "xCJK UNIFIED IDEOGRAPH-4E2D");

        buf.clear();
        assert!(write_name('가', &mut buf));
        assert_eq!(buf, "HANGUL SYLLABLE GA");

        buf.clear();
        assert!(!write_name('\u{378}', &mut buf));
        assert_eq!(buf, "");
    }

    #[test]
    fn write_hex_handles_the_boundaries() {
        let mut digits = [0u8; 8];
        assert_eq!(write_hex(&mut digits, 0, 0), 1);
        assert_eq!(&digits[..1], b"0");
        assert_eq!(write_hex(&mut digits, 0, 4), 4);
        assert_eq!(&digits[..4], b"0000");
        assert_eq!(write_hex(&mut digits, 0x4E2D, 0), 4);
        assert_eq!(&digits[..4], b"4E2D");
        // `EGYPTIAN HIEROGLYPH-13460` needs five digits where the width asks for four.
        assert_eq!(write_hex(&mut digits, 0x13460, 4), 5);
        assert_eq!(&digits[..5], b"13460");
    }

    /// Prints every section's size, then holds the table to the current reference encoder's
    /// limit. [`TABLE_BUDGET`] is the target for the compressed encoder, not this encoder.
    #[test]
    fn table_stays_within_the_encoder_limit() {
        let measured = [
            ("names", sections::NAMES),
            ("name runs", sections::NAME_RUNS),
            ("name patterns", sections::NAME_PATTERNS),
            ("general category", sections::GC),
            ("emoji", sections::EMOJI),
        ];
        for (label, id) in measured {
            let length = tables::section(id).map_or(0, <[u8]>::len);
            eprintln!("{label:>18}: {length:>9} bytes");
        }
        eprintln!(
            "{:>18}: {:>9} bytes (budget {})",
            "total",
            tables::TABLE.len(),
            TABLE_BUDGET
        );

        // The v0 reference encoder is deliberately uncompressed, so the limit it is held to is
        // the encoder's, not the budget's.
        assert!(
            tables::TABLE.len() <= ENCODER_TABLE_LIMIT,
            "table grew to {} bytes",
            tables::TABLE.len()
        );
    }

    /// Times every public lookup over its whole domain, so a layout change can be compared on
    /// both architectures:
    ///
    /// ```sh
    /// cargo test -p zeta-unicode --release -- --ignored --nocapture benchmarks
    /// ```
    #[test]
    #[ignore = "benchmark"]
    fn benchmarks() {
        use std::time::{Duration, Instant};

        /// Named characters per name run, and emoji probes per emoji run.
        const SAMPLES: u32 = 1 << 16;
        /// Every code point in the Unicode range.
        const ALL_CODE_POINTS: u32 = 0x0011_0000;

        fn per_lookup(elapsed: Duration, samples: u32) -> f64 {
            elapsed.as_secs_f64() * 1e9 / f64::from(samples)
        }

        // Every third scalar value below U+3000: dense in the named ranges, and it skips the
        // surrogates `char::from_u32` rejects.
        let points: Vec<char> = (0..SAMPLES)
            .filter_map(|i| char::from_u32(0x20 + i))
            .filter(|c| u32::from(*c) < 0x3000)
            .collect();
        let named = u32::try_from(points.len()).expect("fewer than u32::MAX characters");

        // Three warm-up rounds first: the opening passes pay the page faults.
        for round in 0..4 {
            let start = Instant::now();
            let mut sink = 0u64;
            for c in &points {
                sink += name(*c).map_or(0, |found| found.len() as u64);
            }
            if round == 3 {
                eprintln!(
                    "name:             {:>8.2} ns/lookup (sink {sink})",
                    per_lookup(start.elapsed(), named)
                );
            }
        }

        for round in 0..4 {
            let start = Instant::now();
            let mut sink = 0u64;
            for cp in 0..ALL_CODE_POINTS {
                let c = char::from_u32(cp).unwrap_or(' ');
                sink += general_category(c).short_name().len() as u64;
            }
            if round == 3 {
                eprintln!(
                    "general_category: {:>8.2} ns/lookup (sink {sink})",
                    per_lookup(start.elapsed(), ALL_CODE_POINTS)
                );
            }
        }

        // The interesting emoji shapes: canonical hit, named sequence, synthesized tone
        // variant, tone fallback with two tones, a miss, and a long tag sequence.
        let probes = ["🏳️‍🌈", "2️⃣", "👨🏻‍❤️‍💋‍👨🏻", "🫶🏻", "🧑🏽‍🦰", "🐱‍🏍", "d'être", "🏴󠁧󠁢󠁥󠁮󠁧󠁿"];
        let probe_count = u32::try_from(probes.len()).expect("a handful of probes");
        for round in 0..4 {
            let start = Instant::now();
            let mut sink = 0u64;
            for i in 0..SAMPLES {
                for probe in probes {
                    sink += emoji_name(probe).map_or(0, |found| found.len() as u64);
                    std::hint::black_box(i);
                }
            }
            if round == 3 {
                eprintln!(
                    "emoji_name:       {:>8.2} ns/lookup (sink {sink})",
                    per_lookup(start.elapsed(), SAMPLES * probe_count)
                );
            }
        }
    }
}
