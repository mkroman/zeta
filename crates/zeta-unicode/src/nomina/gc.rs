//! Nomina general-category lookup.
//!
//! The section stores 4,202 contiguous runs as two Huffman streams: varint start gaps and
//! category ids. [`Gc::parse`] decodes them once and rebuilds the incumbent two-level page
//! table (a `u16` directory per 2,048-code-point page over packed `u16` records), so a lookup
//! keeps the same two bounds checks and 9,494 bytes of heap as the shipped module.
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
        let runs = usize::from(u16_at(section, 0)?);
        let starts_raw = usize::from(u16_at(section, 2)?);
        let starts_coded = usize::from(u16_at(section, 4)?);
        let starts_order = *section.get(6)?;
        let cats_coded = usize::from(u16_at(section, 7)?);
        let cats_order = *section.get(9)?;
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

/// Returns the one-based general-category id of `cp` in `gc`.
#[must_use]
pub fn general_category(gc: &Gc, cp: u32) -> Option<u8> {
    gc.general_category(cp)
}

/// Reads a little-endian `u16`, failing rather than wrapping.
fn u16_at(buf: &[u8], offset: usize) -> Option<u16> {
    let bytes = buf.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal flat Huffman encoder for tests: every symbol gets an eight-bit code.
    fn flat_stream(data: &[u8], order: u8) -> Vec<u8> {
        let context_at = |index: usize| -> u8 {
            if order == 0 || index == 0 {
                0
            } else {
                data[index - 1]
            }
        };
        let mut contexts: Vec<u8> = Vec::new();
        for index in 0..data.len() {
            let context = context_at(index);
            if let Err(slot) = contexts.binary_search(&context) {
                contexts.insert(slot, context);
            }
        }
        let mut out = vec![u8::try_from(contexts.len()).unwrap()];
        let mut maps: Vec<Vec<(u8, u8)>> = Vec::new();
        for &context in &contexts {
            let mut symbols: Vec<u8> = data
                .iter()
                .enumerate()
                .filter(|&(index, _)| context_at(index) == context)
                .map(|(_, &byte)| byte)
                .collect();
            symbols.sort_unstable();
            symbols.dedup();
            out.push(context);
            out.push(u8::try_from(symbols.len()).unwrap());
            let mut map = Vec::new();
            for (code, &symbol) in symbols.iter().enumerate() {
                out.push(symbol);
                out.push(8);
                map.push((symbol, u8::try_from(code).unwrap()));
            }
            maps.push(map);
        }
        let mut bits = Vec::new();
        let mut accumulator = 0u8;
        let mut count = 0u8;
        for (index, &byte) in data.iter().enumerate() {
            let context = context_at(index);
            let slot = contexts.binary_search(&context).unwrap();
            let code = maps[slot]
                .iter()
                .find(|&&(symbol, _)| symbol == byte)
                .unwrap()
                .1;
            for bit in (0..8).rev() {
                accumulator = (accumulator << 1) | ((code >> bit) & 1);
                count += 1;
                if count == 8 {
                    bits.push(accumulator);
                    accumulator = 0;
                    count = 0;
                }
            }
        }
        if count != 0 {
            bits.push(accumulator << (8 - count));
        }
        out.extend_from_slice(&bits);
        out
    }

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
