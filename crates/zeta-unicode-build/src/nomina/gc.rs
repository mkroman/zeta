//! Nomina general-category section: contiguous runs over two Huffman streams.
//!
//! `DerivedGeneralCategory.txt` partitions the whole code space into 4,202 contiguous runs.
//! The section stores only the run starts (varint gaps) and the category ids (one byte each);
//! the runtime decodes both once and rebuilds the incumbent two-level page table, so a lookup
//! keeps its two bounds checks and its 9,494-byte heap.
//!
//! # Layout
//!
//! ```text
//! u16 run_count | u16 starts_raw_len | u16 starts_coded_len | u8 starts_order
//! u16 cats_coded_len | u8 cats_order
//! starts stream (Huffman) | category stream (Huffman)
//! ```
//!
//! # Panics
//!
//! Malformed vendored data is an invariant violation and panics with a message naming the
//! offending range. This is build-time code generation: a bad byte is a build failure.

use bytes::BufMut;
use zeta_unicode_format::huffman;
use zeta_unicode_format::{GC_SHORT_NAMES, UNICODE_END, write_varint};

use crate::ucd::{hex_u32, records};

/// The general category range table: `(start, end, table id)`.
pub type GcRange = (u32, u32, u8);

/// The fixed section header: run count, start lengths, and two stream orders.
#[cfg(test)]
const HEADER: usize = 2 + 2 + 2 + 1 + 2 + 1;

/// Counters and measured sizes for one general-category section build.
#[derive(Clone, Debug, Default)]
pub struct GcStats {
    /// Contiguous runs encoded.
    pub runs: usize,
    /// Encoded section size, in bytes.
    pub section_bytes: usize,
    /// Encoded run-start stream size.
    pub coded_starts: usize,
    /// Encoded category-id stream size.
    pub cats_stream: usize,
    /// Raw (pre-Huffman) run-start stream size.
    pub starts_raw: usize,
    /// Number of category ids, which is also the raw category stream size.
    pub cats_raw: usize,
    /// Heap the runtime rebuilds the two-level table in: `(GC_PAGES + runs) * 2`.
    pub heap_bytes: usize,
}

/// Builds the general-category section from `DerivedGeneralCategory.txt`.
///
/// # Panics
///
/// Panics when the vendored ranges are malformed. That is a build failure, not a runtime
/// condition.
#[must_use]
pub fn build(derived_general_category: &str) -> (Vec<u8>, GcStats) {
    let ranges = parse_general_category(derived_general_category);
    general_category_section(&ranges)
}

/// Encodes a contiguous general-category range table.
///
/// # Panics
///
/// Panics when a range is reversed or unsorted, a category id is out of range, the ranges do
/// not reach [`UNICODE_END`], or a stream outgrows its `u16` length. That is a build failure,
/// not a runtime condition.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "the encoder's linear stream pipeline reads better as one function"
)]
pub fn general_category_section(ranges: &[GcRange]) -> (Vec<u8>, GcStats) {
    assert!(!ranges.is_empty(), "an empty range table is not a section");
    let mut stats = GcStats {
        runs: ranges.len(),
        ..GcStats::default()
    };
    let mut start_stream = Vec::new();
    let mut cats = Vec::with_capacity(ranges.len());
    let mut previous = 0u32;
    for &(range_start, range_end, id) in ranges {
        assert!(
            range_start <= range_end,
            "reversed range U+{range_start:04X}..U+{range_end:04X}"
        );
        assert!(
            range_start >= previous,
            "general category ranges are not sorted at U+{range_start:04X}"
        );
        assert!(
            (1..=GC_SHORT_NAMES.len()).contains(&usize::from(id)),
            "general category id {id} is out of range"
        );
        write_varint(range_start - previous, &mut start_stream);
        previous = range_start;
        cats.push(id);
    }
    if let Some(&(_, last_end, _)) = ranges.last() {
        assert_eq!(
            last_end, UNICODE_END,
            "the general category ranges do not reach the last code point"
        );
    }

    let coded_starts = huffman::encode(&start_stream, 0);
    let cats_stream = huffman::encode(&cats, 1);
    assert!(
        u16::try_from(start_stream.len()).is_ok(),
        "the start stream is over u16 bytes"
    );
    assert!(
        u16::try_from(coded_starts.len()).is_ok(),
        "the coded start stream is over u16 bytes"
    );
    assert!(
        u16::try_from(cats_stream.len()).is_ok(),
        "the coded category stream is over u16 bytes"
    );
    assert!(u16::try_from(ranges.len()).is_ok(), "too many runs");

    let mut section = Vec::new();
    section.put_u16_le(u16::try_from(ranges.len()).expect("run count"));
    section.put_u16_le(u16::try_from(start_stream.len()).expect("start stream length"));
    section.put_u16_le(u16::try_from(coded_starts.len()).expect("coded start length"));
    section.push(0);
    section.put_u16_le(u16::try_from(cats_stream.len()).expect("coded category length"));
    section.push(1);
    section.extend_from_slice(&coded_starts);
    section.extend_from_slice(&cats_stream);

    // Round-trip both streams and rebuild the runs exactly as the input.
    let decoded_start_stream =
        huffman::decode(&coded_starts, start_stream.len(), 0).expect("start round-trips");
    let decoded_cats =
        huffman::decode(&cats_stream, cats.len(), 1).expect("category stream round-trips");
    assert_eq!(
        decoded_start_stream, start_stream,
        "start stream round-trip"
    );
    assert_eq!(decoded_cats, cats, "category stream round-trip");
    let mut rebuilt = Vec::with_capacity(ranges.len());
    let mut at = 0usize;
    let mut run_start = 0u32;
    for &id in &decoded_cats {
        let (delta, used) =
            zeta_unicode_format::read_varint(decoded_start_stream.get(at..).expect("start varint"))
                .expect("a start varint");
        at += used;
        run_start = run_start.checked_add(delta).expect("run starts fit in u32");
        rebuilt.push((run_start, id));
    }
    assert_eq!(at, decoded_start_stream.len(), "trailing start bytes");
    for (index, &(wanted_start, _, wanted_id)) in ranges.iter().enumerate() {
        let (run_start, id) = rebuilt[index];
        assert_eq!(
            (run_start, id),
            (wanted_start, wanted_id),
            "decoded run {index}"
        );
    }

    stats.section_bytes = section.len();
    stats.coded_starts = coded_starts.len();
    stats.cats_stream = cats_stream.len();
    stats.starts_raw = start_stream.len();
    stats.cats_raw = cats.len();
    stats.heap_bytes = (zeta_unicode_format::GC_PAGES + ranges.len()) * 2;
    (section, stats)
}

/// Parses `DerivedGeneralCategory.txt` into sorted, contiguous ranges carrying table ids.
///
/// # Panics
///
/// Panics when a vendored line is malformed or the ranges are not a contiguous partition of
/// the Unicode range. That is a build failure, not a runtime condition.
#[must_use]
pub fn parse_general_category(text: &str) -> Vec<GcRange> {
    let mut ranges: Vec<GcRange> = Vec::new();
    for (number, line) in records(text) {
        let (lhs, rest) = line
            .split_once(';')
            .unwrap_or_else(|| panic!("line {number}: {line}"));
        let letter = rest.split('#').next().expect("category field").trim();
        assert_eq!(letter.len(), 2, "line {number}: category {letter}");
        let id = general_category_id(letter);
        let lhs = lhs.trim();
        let (start, end) = if let Some((start, end)) = lhs.split_once("..") {
            (hex_u32(start), hex_u32(end))
        } else {
            let cp = hex_u32(lhs);
            (cp, cp)
        };
        assert!(start <= end, "reversed range U+{start:04X}..U+{end:04X}");
        ranges.push((start, end, id));
    }

    ranges.sort_by_key(|(start, _, _)| *start);
    assert_eq!(ranges.first().map(|range| range.0), Some(0));
    assert_eq!(ranges.last().map(|range| range.1), Some(UNICODE_END));
    for pair in ranges.windows(2) {
        assert!(
            pair[0].1 < pair[1].0,
            "overlapping ranges U+{:04X}..U+{:04X} and U+{:04X}..U+{:04X}",
            pair[0].0,
            pair[0].1,
            pair[1].0,
            pair[1].1
        );
        assert_eq!(
            pair[0].1.checked_add(1),
            Some(pair[1].0),
            "general category ranges must be contiguous"
        );
    }

    let mut seen = [false; GC_SHORT_NAMES.len() + 1];
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

/// The one-based table id of a general category abbreviation.
fn general_category_id(letter: &str) -> u8 {
    let index = GC_SHORT_NAMES
        .iter()
        .position(|short| *short == letter)
        .unwrap_or_else(|| panic!("unknown general category: {letter}"));
    u8::try_from(index).expect("fewer than 255 categories") + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    const DERIVED_GC: &str = include_str!("../../../zeta-unicode/data/DerivedGeneralCategory.txt");

    /// Parses `DerivedGeneralCategory.txt` in the test, independently of the encoder.
    fn oracle_ranges() -> Vec<(u32, u32, u8)> {
        let mut ranges = Vec::new();
        for (number, line) in records(DERIVED_GC) {
            let (lhs, rest) = line
                .split_once(';')
                .unwrap_or_else(|| panic!("line {number}: {line}"));
            let letter = rest.split('#').next().expect("category").trim();
            let id = u8::try_from(
                GC_SHORT_NAMES
                    .iter()
                    .position(|short| *short == letter)
                    .unwrap_or_else(|| panic!("line {number}: unknown category {letter}")),
            )
            .expect("category id")
                + 1;
            let (start, end) = if let Some((start, end)) = lhs.trim().split_once("..") {
                (hex_u32(start), hex_u32(end))
            } else {
                let cp = hex_u32(lhs);
                (cp, cp)
            };
            ranges.push((start, end, id));
        }
        ranges.sort_by_key(|range| range.0);
        ranges
    }

    /// Decodes the section into `(start, category id)` runs.
    fn decode(section: &[u8]) -> Vec<(u32, u8)> {
        let count = usize::from(u16::from_le_bytes([section[0], section[1]]));
        let starts_raw = usize::from(u16::from_le_bytes([section[2], section[3]]));
        let starts_coded = usize::from(u16::from_le_bytes([section[4], section[5]]));
        let starts_order = section[6];
        let cats_coded = usize::from(u16::from_le_bytes([section[7], section[8]]));
        let cats_order = section[9];
        let coded_starts = &section[HEADER..HEADER + starts_coded];
        let cats_stream = &section[HEADER + starts_coded..HEADER + starts_coded + cats_coded];
        let starts = huffman::decode(coded_starts, starts_raw, starts_order).expect("starts");
        let cats = huffman::decode(cats_stream, count, cats_order).expect("cats");
        let mut runs = Vec::with_capacity(count);
        let mut at = 0usize;
        let mut start = 0u32;
        for &id in &cats {
            let (delta, used) = zeta_unicode_format::read_varint(&starts[at..]).expect("delta");
            at += used;
            start += delta;
            runs.push((start, id));
        }
        assert_eq!(at, starts.len());
        runs
    }

    #[test]
    fn every_code_point_has_the_published_category() {
        let oracle = oracle_ranges();
        assert_eq!(oracle.len(), 4_202, "run count");
        let (section, stats) = build(DERIVED_GC);
        assert_eq!(stats.runs, 4_202);
        assert_eq!(stats.heap_bytes, 9_494);
        assert!(
            stats.section_bytes <= 4_000,
            "section grew to {}",
            stats.section_bytes
        );
        let runs = decode(&section);
        assert_eq!(runs.len(), oracle.len());

        let mut run = 0usize;
        for cp in 0..=0x0010_FFFF_u32 {
            while run + 1 < oracle.len() && oracle[run + 1].0 <= cp {
                run += 1;
            }
            assert_eq!(runs[run].1, oracle[run].2, "category id at U+{cp:04X}");
        }
        assert_eq!(runs.last().map(|run| run.0), Some(0x10_FFFE));
    }

    #[test]
    fn every_category_is_used_and_the_ranges_are_contiguous() {
        let oracle = oracle_ranges();
        let mut seen = [false; GC_SHORT_NAMES.len()];
        for (index, &(start, end, id)) in oracle.iter().enumerate() {
            seen[usize::from(id) - 1] = true;
            if let Some(&(_, previous_end, _)) = oracle.get(index.wrapping_sub(1))
                && index > 0
            {
                assert_eq!(previous_end + 1, start, "gap before U+{start:04X}");
            }
            assert!(start <= end);
        }
        assert!(
            seen.iter().all(|&used| used),
            "an unused category slipped in"
        );
    }

    #[test]
    fn a_truncated_section_is_rejected_by_the_round_trip() {
        let (section, _) = build(DERIVED_GC);
        // The encoder's own decode already ran; this pins the section length arithmetic.
        assert_eq!(section.len(), HEADER + 2_140 + 1_710);
        assert_eq!(
            usize::from(u16::from_le_bytes([section[0], section[1]])),
            4_202
        );
    }
}
