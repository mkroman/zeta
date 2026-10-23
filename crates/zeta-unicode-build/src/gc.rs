//! General category parsing and the two-level page-table section encoding.

use zeta_unicode_format::{GC_SHIFT, GC_SHORT_NAMES, UNICODE_END, UNICODE_LIMIT};

/// The general category range table: `(start, end, table id)`.
type GcRanges = Vec<(u32, u32, u8)>;

/// The table id (1..=30) of a general category abbreviation.
fn general_category_id(letter: &str) -> u8 {
    let index = GC_SHORT_NAMES
        .iter()
        .position(|short| *short == letter)
        .unwrap_or_else(|| panic!("unknown general category: {letter}"));
    u8::try_from(index).expect("fewer than 255 categories") + 1
}

/// Parses `DerivedGeneralCategory.txt` into sorted, disjoint ranges carrying table ids.
pub fn parse_general_category(text: &str) -> GcRanges {
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
    // The decoder attributes a gap to the preceding run, so the data must cover the whole
    // scalar range without holes. `DerivedGeneralCategory.txt` includes the `Cn` ranges that
    // make this true; assert it rather than relying on that detail.
    assert_eq!(ranges.first().map(|range| range.0), Some(0));
    assert_eq!(ranges.last().map(|range| range.1), Some(UNICODE_END));
    for pair in ranges.windows(2) {
        assert_eq!(
            pair[0].1.checked_add(1),
            Some(pair[1].0),
            "general category ranges must be contiguous"
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
pub fn general_category_section(ranges: &GcRanges) -> Vec<u8> {
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

    let pages = usize::try_from(UNICODE_LIMIT >> GC_SHIFT).expect("page count fits") + 1;
    let mut directory = Vec::with_capacity(pages * 2);
    let mut index = 0usize;
    for page in 0..pages {
        let base = u32::try_from(page << GC_SHIFT).expect("page base fits in u32");
        while index < ranges.len() && ranges[index].0 < base {
            index += 1;
        }
        crate::table::push_u16(
            &mut directory,
            u16::try_from(index).expect("fewer than 65536 ranges"),
        );
    }

    directory.extend_from_slice(&records);
    directory
}
