//! Nomina `*`-pattern section: varint ranges plus prefix and suffix dictionaries.
//!
//! `DerivedName.txt` defines twenty ranges whose name is `prefix + hex(cp) + suffix`; every
//! suffix is empty in Unicode 18.0.0 and eight prefixes cover all twenty ranges. The section
//! stores each dictionary string once, then one record per range: the start delta, the length
//! and two dictionary ids.
//!
//! # Layout
//!
//! ```text
//! u8 pattern_count | u8 prefix_count | prefix_count * (u8 len, bytes)
//! u8 suffix_count | suffix_count * (u8 len, bytes)
//! pattern_count * (varint start_delta, varint length, u8 prefix_id, u8 suffix_id)
//! ```
//!
//! # Panics
//!
//! Malformed vendored data is an invariant violation and panics with a message naming the
//! offending line. This is build-time code generation: a bad byte is a build failure.

use std::collections::BTreeSet;

use zeta_unicode_format::{UNICODE_END, write_varint};

use crate::ucd::{hex_u32, records};

/// One `*`-pattern range: `prefix + hex(cp) + suffix`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    /// First code point in the range.
    pub lo: u32,
    /// Last code point in the range, inclusive.
    pub hi: u32,
    /// Text before the `*` placeholder.
    pub prefix: String,
    /// Text after the `*` placeholder.
    pub suffix: String,
}

/// Counters and measured sizes for one pattern-section build.
#[derive(Clone, Debug, Default)]
pub struct PatternStats {
    /// Pattern ranges encoded.
    pub patterns: usize,
    /// Encoded section size, in bytes.
    pub section_bytes: usize,
    /// Bytes in the prefix and suffix dictionaries.
    pub dictionary_bytes: usize,
    /// Bytes in the per-range records.
    pub records_bytes: usize,
}

/// Builds the pattern section from `DerivedName.txt`.
///
/// # Panics
///
/// Panics when a vendored line is malformed. That is a build failure, not a runtime condition.
#[must_use]
#[cfg(test)]
pub fn build(derived_names: &str) -> (Vec<u8>, PatternStats) {
    let patterns = parse_patterns(derived_names);
    pattern_section(&patterns)
}

/// Parses the `*`-pattern lines of `DerivedName.txt`, sorted by start code point.
///
/// # Panics
///
/// Panics when a line carries more than one placeholder, a reversed range, or a range past
/// [`UNICODE_END`]. That is a build failure, not a runtime condition.
#[must_use]
pub fn parse_patterns(derived_names: &str) -> Vec<Pattern> {
    let mut patterns = Vec::new();
    for (number, line) in records(derived_names) {
        let (lhs, rhs) = line
            .split_once(';')
            .unwrap_or_else(|| panic!("line {number}: {line}"));
        let rhs = rhs.trim();
        let Some((prefix, rest)) = rhs.split_once('*') else {
            assert!(
                !lhs.contains(".."),
                "line {number}: a range without a '*' placeholder"
            );
            continue;
        };
        assert!(
            !rest.contains('*'),
            "line {number}: more than one '*' placeholder"
        );
        let (lo, hi) = if let Some((lo, hi)) = lhs.trim().split_once("..") {
            (hex_u32(lo), hex_u32(hi))
        } else {
            let cp = hex_u32(lhs);
            (cp, cp)
        };
        assert!(lo <= hi, "line {number}: reversed pattern range");
        assert!(
            hi <= UNICODE_END,
            "line {number}: pattern range U+{lo:04X}..U+{hi:04X} exceeds U+{UNICODE_END:04X}"
        );
        patterns.push(Pattern {
            lo,
            hi,
            prefix: prefix.to_owned(),
            suffix: rest.to_owned(),
        });
    }
    patterns.sort_by_key(|pattern| pattern.lo);
    for pair in patterns.windows(2) {
        assert!(
            pair[0].hi < pair[1].lo,
            "overlapping pattern ranges U+{:04X}..U+{:04X} and U+{:04X}..U+{:04X}",
            pair[0].lo,
            pair[0].hi,
            pair[1].lo,
            pair[1].hi
        );
    }
    patterns
}

/// Encodes a sorted pattern list.
///
/// The section is decoded back and every range and dictionary entry is asserted to round-trip
/// exactly before it is returned. An empty list encodes the valid count-zero section
/// (`[0, 0, 0]`), which the runtime treats as matching nothing.
///
/// # Panics
///
/// Panics when a range is unsorted or reversed, a dictionary string is over 255 bytes, the
/// list is longer than 255 entries, or the encoded section does not decode back to
/// `patterns`. That is a build failure, not a runtime condition.
#[must_use]
pub fn pattern_section(patterns: &[Pattern]) -> (Vec<u8>, PatternStats) {
    let mut stats = PatternStats {
        patterns: patterns.len(),
        ..PatternStats::default()
    };
    assert!(u8::try_from(patterns.len()).is_ok(), "too many patterns");

    let prefixes: Vec<&str> = patterns
        .iter()
        .map(|pattern| pattern.prefix.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let suffixes: Vec<&str> = patterns
        .iter()
        .map(|pattern| pattern.suffix.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    assert!(
        u8::try_from(prefixes.len()).is_ok(),
        "too many pattern prefixes"
    );
    assert!(
        u8::try_from(suffixes.len()).is_ok(),
        "too many pattern suffixes"
    );

    let mut section = Vec::new();
    section.push(u8::try_from(patterns.len()).expect("pattern count"));
    let mut dictionary_bytes = 0usize;
    for dictionary in [&prefixes, &suffixes] {
        section.push(u8::try_from(dictionary.len()).expect("dictionary count"));
        for text in dictionary {
            assert!(
                u8::try_from(text.len()).is_ok(),
                "dictionary string is over 255 bytes"
            );
            section.push(u8::try_from(text.len()).expect("dictionary length"));
            section.extend_from_slice(text.as_bytes());
            dictionary_bytes += 1 + text.len();
        }
    }

    let mut records = Vec::new();
    let mut previous = 0u32;
    for pattern in patterns {
        assert!(
            pattern.lo >= previous,
            "pattern ranges are not sorted at U+{:04X}",
            pattern.lo
        );
        assert!(pattern.hi >= pattern.lo, "reversed pattern range");
        let prefix = prefixes
            .iter()
            .position(|text| **text == pattern.prefix)
            .expect("the prefix was collected");
        let suffix = suffixes
            .iter()
            .position(|text| **text == pattern.suffix)
            .expect("the suffix was collected");
        write_varint(u64::from(pattern.lo - previous), &mut records);
        write_varint(u64::from(pattern.hi - pattern.lo) + 1, &mut records);
        records.push(u8::try_from(prefix).expect("prefix id"));
        records.push(u8::try_from(suffix).expect("suffix id"));
        previous = pattern.lo;
    }
    section.extend_from_slice(&records);

    stats.section_bytes = section.len();
    stats.dictionary_bytes = dictionary_bytes;
    stats.records_bytes = records.len();
    assert_round_trip(&section, patterns);
    (section, stats)
}

/// A decoded pattern section: both dictionaries and every range with its dictionary ids.
struct DecodedSection {
    prefixes: Vec<String>,
    suffixes: Vec<String>,
    /// `(start, end, prefix id, suffix id)` per record, in section order.
    ranges: Vec<(u32, u32, u8, u8)>,
}

/// Decodes a pattern section back into its dictionaries and ranges.
///
/// # Panics
///
/// Panics when the section is truncated or a dictionary string is not UTF-8: a malformed
/// section is a build failure, not a runtime condition.
fn decode_section(section: &[u8]) -> DecodedSection {
    let mut at = 1usize;
    let count = usize::from(section[0]);
    let mut dictionaries = Vec::with_capacity(2);
    for _ in 0..2 {
        let entries = usize::from(section[at]);
        at += 1;
        let mut dictionary = Vec::with_capacity(entries);
        for _ in 0..entries {
            let length = usize::from(section[at]);
            at += 1;
            dictionary.push(
                std::str::from_utf8(&section[at..at + length])
                    .expect("UTF-8")
                    .to_owned(),
            );
            at += length;
        }
        dictionaries.push(dictionary);
    }
    let mut ranges = Vec::with_capacity(count);
    let mut start = 0u32;
    for _ in 0..count {
        let (delta, used) =
            zeta_unicode_format::read_varint(section.get(at..).expect("a start varint"))
                .expect("a start varint");
        at += used;
        start = start.checked_add(delta).expect("pattern starts fit in u32");
        let (length, used) =
            zeta_unicode_format::read_varint(section.get(at..).expect("a length varint"))
                .expect("a length varint");
        at += used;
        let end = start
            .checked_add(length.checked_sub(1).expect("a pattern length of zero"))
            .expect("a pattern end fits in u32");
        ranges.push((start, end, section[at], section[at + 1]));
        at += 2;
    }
    assert_eq!(at, section.len(), "trailing pattern-section bytes");
    let [prefixes, suffixes] = dictionaries.try_into().expect("two dictionaries");
    DecodedSection {
        prefixes,
        suffixes,
        ranges,
    }
}

/// Asserts the section decodes back to exactly `patterns`.
///
/// # Panics
///
/// Panics on the first range or dictionary entry that does not match: a mismatch is a build
/// failure, not a runtime condition.
fn assert_round_trip(section: &[u8], patterns: &[Pattern]) {
    let decoded = decode_section(section);
    assert_eq!(
        decoded.ranges.len(),
        patterns.len(),
        "the pattern count does not round-trip"
    );
    for (index, pattern) in patterns.iter().enumerate() {
        let (start, end, prefix_id, suffix_id) = decoded.ranges[index];
        assert_eq!(start, pattern.lo, "pattern {index} start");
        assert_eq!(end, pattern.hi, "pattern {index} end");
        assert_eq!(
            decoded
                .prefixes
                .get(usize::from(prefix_id))
                .map(String::as_str),
            Some(pattern.prefix.as_str()),
            "pattern {index} prefix"
        );
        assert_eq!(
            decoded
                .suffixes
                .get(usize::from(suffix_id))
                .map(String::as_str),
            Some(pattern.suffix.as_str()),
            "pattern {index} suffix"
        );
    }
    let wanted_prefixes: BTreeSet<&str> = patterns
        .iter()
        .map(|pattern| pattern.prefix.as_str())
        .collect();
    let wanted_suffixes: BTreeSet<&str> = patterns
        .iter()
        .map(|pattern| pattern.suffix.as_str())
        .collect();
    assert_eq!(
        decoded
            .prefixes
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        wanted_prefixes,
        "the prefix dictionary does not round-trip"
    );
    assert_eq!(
        decoded
            .suffixes
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        wanted_suffixes,
        "the suffix dictionary does not round-trip"
    );
}

#[cfg(test)]
mod tests {
    use zeta_unicode_format::write_pattern_name;

    use super::*;

    const DERIVED: &str = include_str!("../../../zeta-unicode/data/DerivedName.txt");

    /// Parses the `*`-pattern records in the test, independently of the encoder.
    fn oracle_patterns() -> Vec<(u32, u32, String, String)> {
        let mut patterns = Vec::new();
        for (number, line) in records(DERIVED) {
            let (lhs, rhs) = line
                .split_once(';')
                .unwrap_or_else(|| panic!("line {number}: {line}"));
            let rhs = rhs.trim();
            let Some((prefix, suffix)) = rhs.split_once('*') else {
                continue;
            };
            assert!(!suffix.contains('*'), "line {number}: two placeholders");
            let (lo, hi) = if let Some((lo, hi)) = lhs.trim().split_once("..") {
                (hex_u32(lo), hex_u32(hi))
            } else {
                let cp = hex_u32(lhs);
                (cp, cp)
            };
            patterns.push((lo, hi, prefix.to_owned(), suffix.to_owned()));
        }
        patterns.sort_by_key(|pattern| pattern.0);
        patterns
    }

    #[test]
    fn every_pattern_range_is_encoded_and_materializes() {
        let oracle = oracle_patterns();
        assert_eq!(oracle.len(), 20, "pattern count");
        let (section, stats) = build(DERIVED);
        assert_eq!(stats.patterns, 20);
        assert!(
            stats.section_bytes <= 700,
            "section grew to {}",
            stats.section_bytes
        );
        let decoded = decode_section(&section);
        assert_eq!(decoded.prefixes.len(), 8);
        assert_eq!(decoded.suffixes.len(), 1);
        assert_eq!(decoded.ranges.len(), oracle.len());

        for (index, &(lo, hi, ref prefix, ref suffix)) in oracle.iter().enumerate() {
            let (start, end, prefix_id, suffix_id) = decoded.ranges[index];
            assert_eq!(start, lo, "range start {index}");
            assert_eq!(end, hi, "range end {index}");
            assert_eq!(
                decoded.prefixes[usize::from(prefix_id)],
                *prefix,
                "prefix {index}"
            );
            assert_eq!(
                decoded.suffixes[usize::from(suffix_id)],
                *suffix,
                "suffix {index}"
            );
            for cp in lo..=hi {
                let mut want = String::new();
                write_pattern_name(prefix, suffix, cp, &mut want);
                let mut got = String::new();
                write_pattern_name(
                    &decoded.prefixes[usize::from(prefix_id)],
                    &decoded.suffixes[usize::from(suffix_id)],
                    cp,
                    &mut got,
                );
                assert_eq!(got, want, "pattern name for U+{cp:04X}");
            }
        }
    }

    #[test]
    fn the_placeholder_widens_past_four_digits() {
        let (section, _) = build(DERIVED);
        let decoded = decode_section(&section);
        let range = decoded
            .ranges
            .iter()
            .find(|range| (range.0..=range.1).contains(&0x18CFF))
            .expect("the single-code-point pattern");
        assert_eq!(
            decoded.prefixes[usize::from(range.2)],
            "KHITAN SMALL SCRIPT CHARACTER-"
        );
        let range = decoded
            .ranges
            .iter()
            .find(|range| (range.0..=range.1).contains(&0x13460))
            .expect("the Egyptian range");
        assert_eq!(
            decoded.prefixes[usize::from(range.2)],
            "EGYPTIAN HIEROGLYPH-"
        );
    }

    #[test]
    fn non_pattern_lines_are_ignored() {
        let text = "\
0041; LATIN CAPITAL LETTER A
3400..3401; TEST-*
4E00; CJK UNIFIED IDEOGRAPH-4E00
";
        let patterns = parse_patterns(text);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].lo, 0x3400);
        assert_eq!(patterns[0].hi, 0x3401);
        assert_eq!(patterns[0].prefix, "TEST-");
        assert_eq!(patterns[0].suffix, "");
    }

    #[test]
    fn an_empty_pattern_list_is_the_count_zero_section() {
        let (section, stats) = pattern_section(&[]);
        assert_eq!(section, [0, 0, 0]);
        assert_eq!(stats.patterns, 0);
        assert_eq!(stats.section_bytes, 3);
        assert_eq!(stats.dictionary_bytes, 0);
        assert_eq!(stats.records_bytes, 0);

        let decoded = decode_section(&section);
        assert_eq!(decoded.prefixes, Vec::<String>::new());
        assert_eq!(decoded.suffixes, Vec::<String>::new());
        assert_eq!(decoded.ranges, Vec::new());
    }

    #[test]
    #[should_panic(expected = "line 2: pattern range U+10FFFF..U+110001 exceeds U+10FFFF")]
    fn a_pattern_range_past_unicode_is_rejected() {
        let text = "\
0041; LATIN CAPITAL LETTER A
10FFFF..110001; TEST-*
";
        let _ = parse_patterns(text);
    }
}
