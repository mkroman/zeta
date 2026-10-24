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
pub fn build(derived_names: &str) -> (Vec<u8>, PatternStats) {
    let patterns = parse_patterns(derived_names);
    pattern_section(&patterns)
}

/// Parses the `*`-pattern lines of `DerivedName.txt`, sorted by start code point.
///
/// # Panics
///
/// Panics when a line carries more than one placeholder or a reversed range. That is a build
/// failure, not a runtime condition.
#[must_use]
pub fn parse_patterns(derived_names: &str) -> Vec<Pattern> {
    let mut patterns = Vec::new();
    for (number, line) in records(derived_names) {
        let (lhs, rhs) = line
            .split_once(';')
            .unwrap_or_else(|| panic!("line {number}: {line}"));
        let rhs = rhs.trim();
        let Some((prefix, rest)) = rhs.split_once('*') else {
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
/// # Panics
///
/// Panics when a range is unsorted or reversed, a dictionary string is over 255 bytes, or the
/// list is longer than 255 entries. That is a build failure, not a runtime condition.
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

    assert!(
        !patterns.is_empty(),
        "an empty pattern list is not a section"
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
    (section, stats)
}

/// The non-empty, non-comment records of a UCD file, with 1-based line numbers.
fn records(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .map(|(index, line)| (index + 1, line))
}

/// Parses one hexadecimal Unicode code point.
fn hex_u32(text: &str) -> u32 {
    u32::from_str_radix(text.trim(), 16)
        .unwrap_or_else(|error| panic!("invalid hexadecimal code point {text:?}: {error}"))
}

/// Appends a LEB128 varint.
fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        #[allow(clippy::cast_possible_truncation, reason = "the low seven bits")]
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    #[allow(clippy::cast_possible_truncation, reason = "the remaining byte")]
    out.push(value as u8);
}

#[cfg(test)]
mod tests {
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

    /// A decoded section: `(prefix, suffix)` dictionaries and ranges with their ids.
    struct Decoded {
        prefixes: Vec<String>,
        suffixes: Vec<String>,
        ranges: Vec<(u32, u32, u8, u8)>,
    }

    fn decode(section: &[u8]) -> Decoded {
        let mut at = 0usize;
        let count = usize::from(section[at]);
        at += 1;
        let mut dictionaries = Vec::new();
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
            let (delta, used) = zeta_unicode_format::read_varint(&section[at..]).expect("delta");
            at += used;
            start += delta;
            let (length, used) = zeta_unicode_format::read_varint(&section[at..]).expect("length");
            at += used;
            ranges.push((start, start + length - 1, section[at], section[at + 1]));
            at += 2;
        }
        assert_eq!(at, section.len());
        Decoded {
            prefixes: dictionaries.remove(0),
            suffixes: dictionaries.remove(0),
            ranges,
        }
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
        let decoded = decode(&section);
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
                let mut want = prefix.clone();
                let mut digits = [0u8; 8];
                let used = zeta_unicode_format::write_hex(&mut digits, cp, 0);
                want.push_str(std::str::from_utf8(&digits[..used]).expect("ASCII"));
                want.push_str(suffix);
                let mut got = decoded.prefixes[usize::from(prefix_id)].clone();
                let used = zeta_unicode_format::write_hex(&mut digits, cp, 0);
                got.push_str(std::str::from_utf8(&digits[..used]).expect("ASCII"));
                got.push_str(&decoded.suffixes[usize::from(suffix_id)]);
                assert_eq!(got, want, "pattern name for U+{cp:04X}");
            }
        }
    }

    #[test]
    fn the_placeholder_widens_past_four_digits() {
        let (section, _) = build(DERIVED);
        let decoded = decode(&section);
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
}
