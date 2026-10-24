//! Nomina general-category lookup.
//!
//! The section stores 4,202 contiguous runs as two Huffman streams: varint start gaps and
//! category ids. [`Gc::parse`] decodes them once and rebuilds a two-level page table (a `u16`
//! directory per 2,048-code-point page over packed `u16` records), so a lookup keeps two bounds
//! checks and 9,494 bytes of heap.
//!
//! # Layout
//!
//! ```text
//! u16 run_count | u16 starts_raw_len | u16 starts_coded_len | u8 starts_order
//! u16 cats_coded_len | u8 cats_order
//! starts stream (Huffman) | category stream (Huffman)
//! ```
//!
//! A record packs the page-relative run start in the high bits and the one-based category id
//! in the low [`GC_ID_BITS`] bits.

use bytes::Buf;
use zeta_unicode_format::{
    GC_ID_BITS, GC_ID_MASK, GC_PAGES, GC_SHIFT, GC_SHORT_NAMES, UNICODE_END,
};

use super::emoji::huffman;

/// The parsed general-category section.
pub struct Gc {
    /// First record index of each 2,048-code-point page, plus one sentinel.
    directory: Vec<u16>,
    /// Packed page-relative run start and category id, sorted by start.
    records: Vec<u16>,
}

/// The fixed section header: run count, start lengths, and two stream orders.
const HEADER: usize = 2 + 2 + 2 + 1 + 2 + 1;

impl Gc {
    /// Parses and decodes a general-category section.
    ///
    /// Returns `None` for a truncated or inconsistent section.
    #[must_use]
    pub fn parse(section: &[u8]) -> Option<Self> {
        let mut header = section;
        let runs = usize::from(header.try_get_u16_le().ok()?);
        let starts_raw = usize::from(header.try_get_u16_le().ok()?);
        let starts_coded = usize::from(header.try_get_u16_le().ok()?);
        let starts_order = header.try_get_u8().ok()?;
        let cats_coded = usize::from(header.try_get_u16_le().ok()?);
        let cats_order = header.try_get_u8().ok()?;
        if runs == 0 || starts_order > 1 || cats_order > 1 {
            return None;
        }

        let starts_at = HEADER;
        let cats_at = starts_at.checked_add(starts_coded)?;
        let end = cats_at.checked_add(cats_coded)?;
        if end != section.len() {
            return None;
        }
        let starts_stream = section.get(starts_at..cats_at)?;
        let cats_stream = section.get(cats_at..end)?;
        let starts = huffman::decode(starts_stream, starts_raw, starts_order)?;
        let cats = huffman::decode(cats_stream, runs, cats_order)?;
        if cats.len() != runs {
            return None;
        }

        let mut starts_list = Vec::with_capacity(runs);
        let mut at = 0usize;
        let mut start = 0u32;
        for _ in 0..runs {
            let (delta, used) = zeta_unicode_format::read_varint(starts.get(at..)?)?;
            at += used;
            start = start.checked_add(delta)?;
            if start > UNICODE_END {
                return None;
            }
            if starts_list.last().is_some_and(|&previous| start < previous) {
                return None;
            }
            starts_list.push(start);
        }
        if at != starts.len() {
            return None;
        }

        let mut records = Vec::with_capacity(runs);
        for (&start, &id) in starts_list.iter().zip(&cats) {
            let id = usize::from(id);
            if id == 0 || id > GC_SHORT_NAMES.len() {
                return None;
            }
            let page = start >> GC_SHIFT;
            let relative = start - (page << GC_SHIFT);
            let record = (relative << GC_ID_BITS) | u32::try_from(id).ok()?;
            records.push(u16::try_from(record).ok()?);
        }
        let mut directory = Vec::with_capacity(GC_PAGES);
        let mut index = 0usize;
        for page in 0..GC_PAGES {
            let base = u32::try_from(page).ok()?.checked_shl(GC_SHIFT)?;
            while index < starts_list.len()
                && starts_list.get(index).copied().unwrap_or(u32::MAX) < base
            {
                index += 1;
            }
            directory.push(u16::try_from(index).ok()?);
        }

        Some(Self { directory, records })
    }

    /// Returns the number of encoded runs.
    #[must_use]
    #[cfg(test)]
    pub const fn run_count(&self) -> usize {
        self.records.len()
    }

    /// Returns the one-based category id of `cp`, or `None` when `cp` is not a Unicode code
    /// point or falls before the first run.
    #[must_use]
    pub fn general_category(&self, cp: u32) -> Option<u8> {
        if cp > UNICODE_END {
            return None;
        }
        let page = usize::try_from(cp >> GC_SHIFT).ok()?;
        let relative = cp & ((1 << GC_SHIFT) - 1);
        let first = usize::from(*self.directory.get(page)?);
        let end = usize::from(*self.directory.get(page.checked_add(1)?)?);
        let (mut low, mut high) = (first, end);
        while low < high {
            let middle = low + (high - low) / 2;
            let start = u32::from(*self.records.get(middle)? >> GC_ID_BITS);
            if start <= relative {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let record = *self.records.get(low.checked_sub(1)?)?;
        let id = usize::from(record & GC_ID_MASK);
        if id == 0 || id > GC_SHORT_NAMES.len() {
            return None;
        }
        u8::try_from(id).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::flat_stream;
    use super::*;

    /// Builds a section from explicit `(start, category id)` runs.
    fn section(runs: &[(u32, u8)]) -> Vec<u8> {
        let mut starts = Vec::new();
        let mut previous = 0u32;
        for &(start, _) in runs {
            zeta_unicode_format::write_varint(u64::from(start - previous), &mut starts);
            previous = start;
        }
        let cats: Vec<u8> = runs.iter().map(|&(_, id)| id).collect();
        let starts_stream = flat_stream(&starts, 0);
        let cats_stream = flat_stream(&cats, 1);
        let mut out = Vec::new();
        out.extend_from_slice(&u16::try_from(runs.len()).unwrap().to_le_bytes());
        out.extend_from_slice(&u16::try_from(starts.len()).unwrap().to_le_bytes());
        out.extend_from_slice(&u16::try_from(starts_stream.len()).unwrap().to_le_bytes());
        out.push(0);
        out.extend_from_slice(&u16::try_from(cats_stream.len()).unwrap().to_le_bytes());
        out.push(1);
        out.extend_from_slice(&starts_stream);
        out.extend_from_slice(&cats_stream);
        out
    }

    #[test]
    fn runs_resolve_with_carry_over_across_pages() {
        let blob = section(&[(0, 1), (0x80, 2), (0x700, 3), (0x900, 4), (0x10_FF00, 30)]);
        let gc = Gc::parse(&blob).expect("the section parses");
        assert_eq!(gc.run_count(), 5);
        assert_eq!(gc.general_category(0), Some(1));
        assert_eq!(gc.general_category(0x7F), Some(1));
        assert_eq!(gc.general_category(0x80), Some(2));
        assert_eq!(gc.general_category(0x6FF), Some(2));
        assert_eq!(gc.general_category(0x700), Some(3));
        assert_eq!(gc.general_category(0x7FF), Some(3));
        assert_eq!(gc.general_category(0x800), Some(3));
        assert_eq!(gc.general_category(0x900), Some(4));
        assert_eq!(gc.general_category(0x10_FEFF), Some(4));
        assert_eq!(gc.general_category(0x10_FF00), Some(30));
        assert_eq!(gc.general_category(0x10_FFFF), Some(30));
        assert_eq!(gc.general_category(0x11_0000), None);
    }

    #[test]
    fn truncated_sections_are_rejected() {
        let blob = section(&[(0, 1), (0x80, 2)]);
        assert!(Gc::parse(&blob).is_some());
        for length in 0..blob.len() {
            assert!(Gc::parse(&blob[..length]).is_none(), "length {length}");
        }
        assert!(Gc::parse(&[]).is_none());
    }
}
