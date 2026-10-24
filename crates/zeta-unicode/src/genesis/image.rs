//! The flat Genesis image and its four sections.
//!
//! [`Genesis::new`] validates the 64-byte header once and keeps a slice per section; every
//! lookup then reads those slices directly. There is no startup decode, no `OnceLock` and no
//! heap allocation in a name lookup.
//!
//! Byte 13 of the header selects the name layout: 0 is the block-local front-coding layout,
//! 1 is the global checkpointed layout. Both are decoded by [`super::names`]; the runtime
//! accepts either.
//!
//! ```text
//! off  field
//!   0  magic "ZGL1"
//!   4  Unicode version, three u16
//!  10  section count (4)
//!  11  name block shift (5 = 32 names, 6 = 64)
//!  12  general-category page shift (11)
//!  13  name layout (0 block-local, 1 global checkpointed)
//!  14  4 x (payload offset u32, payload length u32)
//!  46  padding to 64 bytes
//! ```
//!
//! # Pattern section
//!
//! Section 3 is the `*`-pattern table written by `zeta_unicode_build::genesis::patterns`:
//! a pattern count, a prefix dictionary, a suffix dictionary, then one record per range
//! (`varint start delta`, `varint length`, `u8 prefix id`, `u8 suffix id`). Pattern names are
//! `prefix + upper-case hex(cp) + suffix`.

use std::borrow::Cow;

use zeta_unicode_format::{UNICODE_END, read_varint};

use super::names::{self, Layout, Names};

/// The image magic.
pub const MAGIC: &[u8; 4] = b"ZGL1";
/// The number of sections in the image.
pub const SECTION_COUNT: usize = 4;
/// The section id of the stored names.
pub const NAMES: usize = 0;
/// The section id of the emoji entries.
pub const EMOJI: usize = 1;
/// The section id of the general category.
pub const GC: usize = 2;
/// The section id of the `*` patterns.
pub const PATTERNS: usize = 3;
/// The size of the padded image header.
pub const HEADER_LEN: usize = 64;
/// The name-block shift the shipped image uses.
pub const NAME_BLOCK_SHIFT: u8 = 5;
/// The general-category page shift the shipped image uses.
pub const GC_PAGE_SHIFT: u8 = 11;

/// A parsed Genesis image.
pub struct Genesis {
    /// The four section payloads, in id order.
    sections: [Option<&'static [u8]>; SECTION_COUNT],
    /// The decoded name section.
    names: Names,
    /// The `*`-pattern section.
    patterns: Patterns,
}

impl Genesis {
    /// Parses a Genesis image.
    ///
    /// Returns `None` when the magic, section count, header shifts, section bounds or the
    /// name section are invalid. The image must live for `'static` because every returned
    /// name borrows from it.
    #[must_use]
    pub fn new(table: &'static [u8]) -> Option<Self> {
        if table.get(..4) != Some(MAGIC.as_slice()) {
            return None;
        }
        let section_count = *table.get(10)?;
        if usize::from(section_count) != SECTION_COUNT {
            return None;
        }
        let block_shift = *table.get(11)?;
        let page_shift = *table.get(12)?;
        if page_shift != GC_PAGE_SHIFT {
            return None;
        }
        let layout = Layout::from_id(*table.get(13)?)?;

        let mut sections = [None; SECTION_COUNT];
        for (id, slot) in sections.iter_mut().enumerate() {
            let at = 14 + id * 8;
            let offset = usize::try_from(u32_at(table, at)?).ok()?;
            let length = usize::try_from(u32_at(table, at + 4)?).ok()?;
            let end = offset.checked_add(length)?;
            *slot = Some(table.get(offset..end)?);
        }

        let names = Names::new(sections[NAMES]?, block_shift, layout)?;
        let patterns = Patterns::new(sections[PATTERNS]?)?;
        Some(Self {
            sections,
            names,
            patterns,
        })
    }

    /// Returns the payload of section `id`, or `None` when there is no such section.
    #[must_use]
    pub fn section(&self, id: usize) -> Option<&'static [u8]> {
        self.sections.get(id).copied().flatten()
    }

    /// Appends the name of `cp` to `out`, returning `false` when it has no name.
    ///
    /// This is the zero-allocation path: the only buffer is the name section's fixed
    /// 256-byte scratch array. A stored name decodes at most one block.
    #[must_use = "the return value reports whether the code point has a name"]
    pub fn write_name(&mut self, cp: u32, out: &mut String) -> bool {
        if let Some(id) = self.names.id_of(cp) {
            return self.names.write_id(id, out);
        }
        if names::write_hangul(cp, out) {
            return true;
        }
        self.patterns.write(cp, out)
    }

    /// Returns the name of `cp`, or `None` when it has none.
    ///
    /// Stored names, Hangul names and pattern names all come back as `Cow::Owned` because
    /// they are decoded into a temporary string. Use [`Self::write_name`] to avoid the
    /// allocation.
    #[must_use]
    pub fn name(&mut self, cp: u32) -> Option<Cow<'static, str>> {
        let mut out = String::new();
        self.write_name(cp, &mut out).then_some(Cow::Owned(out))
    }

    /// Returns the name of `cp`, or the label Unicode defines for code points without one.
    ///
    /// Labels are `<private-use-F8FF>`, `<reserved-0378>`, `<noncharacter-FFFE>` and
    /// `<control-0000>`, exactly as [`crate::name_or_label`] renders them. The category used
    /// for the label is still the shipped general-category table; wiring it to the Genesis
    /// section is the general-category workstream's step.
    #[must_use]
    pub fn name_or_label(&mut self, cp: u32) -> Cow<'static, str> {
        if let Some(name) = self.name(cp) {
            return name;
        }
        let kind = match char::from_u32(cp) {
            Some(c) => match crate::general_category(c) {
                crate::GeneralCategory::PrivateUse => "private-use",
                crate::GeneralCategory::Control => "control",
                crate::GeneralCategory::Surrogate => "surrogate",
                crate::GeneralCategory::Unassigned if crate::is_noncharacter(cp) => "noncharacter",
                _ => "reserved",
            },
            None if (0xD800..=0xDFFF).contains(&cp) => "surrogate",
            None => "reserved",
        };
        Cow::Owned(format!("<{kind}-{cp:04X}>"))
    }
}

/// The `*`-pattern section.
struct Patterns {
    /// Number of ranges.
    count: usize,
    /// Prefix strings, by dictionary id.
    prefixes: Vec<&'static [u8]>,
    /// Suffix strings, by dictionary id.
    suffixes: Vec<&'static [u8]>,
    /// The range records.
    records: &'static [u8],
}

impl Patterns {
    /// Parses a pattern section; an empty section is valid and matches nothing.
    fn new(section: &'static [u8]) -> Option<Self> {
        if section.is_empty() {
            return Some(Self {
                count: 0,
                prefixes: Vec::new(),
                suffixes: Vec::new(),
                records: &[],
            });
        }
        let count = usize::from(*section.first()?);
        let (prefixes, at) = read_dictionary(section, 1)?;
        let (suffixes, at) = read_dictionary(section, at)?;
        let records = section.get(at..)?;

        // Validate every record now, so the lookup path can assume well-formed varints.
        let mut cursor = 0usize;
        let mut previous = 0u32;
        for _ in 0..count {
            let (delta, used) = read_varint(records.get(cursor..)?)?;
            cursor += used;
            let (length, used) = read_varint(records.get(cursor..)?)?;
            cursor += used;
            let prefix = *records.get(cursor)?;
            let suffix = *records.get(cursor + 1)?;
            cursor += 2;
            if length == 0
                || usize::from(prefix) >= prefixes.len()
                || usize::from(suffix) >= suffixes.len()
            {
                return None;
            }
            let start = previous.checked_add(delta)?;
            if start > UNICODE_END || start.checked_add(length - 1)? > UNICODE_END {
                return None;
            }
            previous = start;
        }
        if cursor != records.len() {
            return None;
        }
        Some(Self {
            count,
            prefixes,
            suffixes,
            records,
        })
    }

    /// Appends the pattern name of `cp` to `out`, returning `false` when no range covers it.
    fn write(&self, cp: u32, out: &mut String) -> bool {
        self.write_pattern(cp, out).unwrap_or(false)
    }

    /// The fallible body of [`Self::write`].
    fn write_pattern(&self, cp: u32, out: &mut String) -> Option<bool> {
        let mut cursor = 0usize;
        let mut previous = 0u32;
        for _ in 0..self.count {
            let (delta, used) = read_varint(self.records.get(cursor..)?)?;
            cursor += used;
            let (length, used) = read_varint(self.records.get(cursor..)?)?;
            cursor += used;
            let prefix = *self.prefixes.get(usize::from(*self.records.get(cursor)?))?;
            let suffix = *self
                .suffixes
                .get(usize::from(*self.records.get(cursor + 1)?))?;
            cursor += 2;
            let start = previous.checked_add(delta)?;
            let end = start.checked_add(length)?;
            previous = start;
            if cp < start {
                return Some(false);
            }
            if cp < end {
                out.push_str(std::str::from_utf8(prefix).ok()?);
                push_hex(out, cp);
                out.push_str(std::str::from_utf8(suffix).ok()?);
                return Some(true);
            }
        }
        Some(false)
    }
}

/// Reads a `u8`-counted dictionary of `u8`-length strings, returning it and the next offset.
fn read_dictionary(section: &'static [u8], at: usize) -> Option<(Vec<&'static [u8]>, usize)> {
    let count = usize::from(*section.get(at)?);
    let mut cursor = at + 1;
    let mut dictionary = Vec::with_capacity(count);
    for _ in 0..count {
        let length = usize::from(*section.get(cursor)?);
        cursor += 1;
        dictionary.push(section.get(cursor..cursor + length)?);
        cursor += length;
    }
    Some((dictionary, cursor))
}

/// Reads a little-endian `u32`.
#[inline]
fn u32_at(buf: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        buf.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// Appends the upper-case hex of `cp` with no minimum width, as Unicode writes a `*`
/// placeholder (`U+18CFF` is five digits, `U+4E00` four).
fn push_hex(out: &mut String, cp: u32) {
    let mut digits = [0u8; 8];
    let used = zeta_unicode_format::write_hex(&mut digits, cp, 0);
    out.push_str(std::str::from_utf8(&digits[..used]).expect("hex digits are ASCII"));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds the pattern section format for a synthetic list.
    fn pattern_section(patterns: &[(u32, u32, &str, &str)]) -> Vec<u8> {
        let mut prefixes: Vec<&str> = patterns.iter().map(|record| record.2).collect();
        prefixes.sort_unstable();
        prefixes.dedup();
        let mut suffixes: Vec<&str> = patterns.iter().map(|record| record.3).collect();
        suffixes.sort_unstable();
        suffixes.dedup();

        let mut section = vec![
            u8::try_from(patterns.len()).expect("few patterns"),
            u8::try_from(prefixes.len()).expect("few prefixes"),
        ];
        for prefix in &prefixes {
            section.push(u8::try_from(prefix.len()).expect("short prefix"));
            section.extend_from_slice(prefix.as_bytes());
        }
        section.push(u8::try_from(suffixes.len()).expect("few suffixes"));
        for suffix in &suffixes {
            section.push(u8::try_from(suffix.len()).expect("short suffix"));
            section.extend_from_slice(suffix.as_bytes());
        }
        let mut previous = 0u32;
        for &(lo, hi, prefix, suffix) in patterns {
            write_varint(u64::from(lo - previous), &mut section);
            write_varint(u64::from(hi - lo + 1), &mut section);
            section
                .push(u8::try_from(prefixes.iter().position(|p| *p == prefix).unwrap()).unwrap());
            section
                .push(u8::try_from(suffixes.iter().position(|s| *s == suffix).unwrap()).unwrap());
            previous = lo;
        }
        section
    }

    /// Appends a LEB128 varint.
    fn write_varint(mut value: u64, out: &mut Vec<u8>) {
        while value >= 0x80 {
            out.push(u8::try_from(value & 0x7F).unwrap() | 0x80);
            value >>= 7;
        }
        out.push(u8::try_from(value).unwrap());
    }

    #[test]
    fn patterns_resolve_prefix_hex_and_suffix() {
        let section: &'static [u8] = pattern_section(&[
            (0x3400, 0x3401, "CJK UNIFIED IDEOGRAPH-", ""),
            (0x18CFF, 0x18CFF, "KHITAN SMALL SCRIPT CHARACTER-", "-X"),
        ])
        .leak();
        let patterns = Patterns::new(section).expect("a valid pattern section");

        let mut out = String::new();
        assert!(patterns.write(0x3400, &mut out));
        assert_eq!(out, "CJK UNIFIED IDEOGRAPH-3400");
        out.clear();
        assert!(patterns.write(0x3401, &mut out));
        assert_eq!(out, "CJK UNIFIED IDEOGRAPH-3401");
        out.clear();
        assert!(patterns.write(0x18CFF, &mut out));
        assert_eq!(out, "KHITAN SMALL SCRIPT CHARACTER-18CFF-X");
        out.clear();
        assert!(!patterns.write(0x3402, &mut out));
        assert!(!patterns.write(0x18CFE, &mut out));
        assert_eq!(out, "");
    }

    #[test]
    fn an_empty_pattern_section_is_valid() {
        let patterns = Patterns::new(&[]).expect("empty is valid");
        let mut out = String::new();
        assert!(!patterns.write(0x3400, &mut out));
    }

    #[test]
    fn a_truncated_pattern_section_is_rejected() {
        let section: &'static [u8] =
            pattern_section(&[(0x3400, 0x3401, "CJK UNIFIED IDEOGRAPH-", "")]).leak();
        assert!(Patterns::new(section).is_some());
        assert!(Patterns::new(&section[..section.len() - 1]).is_none());
    }

    /// One flat model: every symbol is an eight-bit code.
    fn flat_model() -> Vec<u8> {
        let mut blob = vec![8];
        for length in 1..=8u16 {
            let count = if length == 8 { 256u16 } else { 0 };
            blob.extend_from_slice(&count.to_le_bytes());
        }
        blob.extend(0..=u8::MAX);
        blob
    }

    /// Builds a name section whose bitstream is exactly `prefix, suffix bytes, 0` per entry.
    fn name_section(entries: &[(u32, String)]) -> Vec<u8> {
        const PAGE_COUNT: usize = 545;
        let block_size = 32usize;
        let block_count = entries.len().div_ceil(block_size);

        let mut stream = Vec::new();
        let mut block_bits = Vec::with_capacity(block_count);
        for block in entries.chunks(block_size) {
            block_bits.push(u32::try_from(stream.len() * 8).expect("bit offset"));
            let mut previous = "";
            for (_, name) in block {
                let prefix = previous
                    .as_bytes()
                    .iter()
                    .zip(name.as_bytes())
                    .take_while(|(left, right)| left == right)
                    .count();
                stream.push(u8::try_from(prefix).expect("short prefix"));
                stream.extend_from_slice(&name.as_bytes()[prefix..]);
                stream.push(0);
                previous = name;
            }
        }

        let mut runs: Vec<(u32, u32, u32)> = Vec::new();
        let mut start = 0usize;
        for id in 1..=entries.len() {
            if id == entries.len() || entries[id].0 != entries[id - 1].0 + 1 {
                runs.push((
                    entries[start].0,
                    u32::try_from(id - start).expect("run length"),
                    u32::try_from(start).expect("base id"),
                ));
                start = id;
            }
        }

        let mut section = Vec::new();
        section.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
        section.extend_from_slice(&u32::try_from(block_count).unwrap().to_le_bytes());
        section.extend_from_slice(&u16::try_from(runs.len()).unwrap().to_le_bytes());
        section.extend_from_slice(&u16::try_from(PAGE_COUNT).unwrap().to_le_bytes());
        let mut directory = Vec::with_capacity(PAGE_COUNT);
        let mut index = 0usize;
        for page in 0..PAGE_COUNT {
            let base = u32::try_from(page).unwrap() << 11;
            while index < runs.len() && runs[index].0 + runs[index].1 <= base {
                index += 1;
            }
            directory.push(u16::try_from(index).unwrap());
        }
        for first in directory {
            section.extend_from_slice(&first.to_le_bytes());
        }
        for (first, length, base) in &runs {
            section.extend_from_slice(&first.to_le_bytes());
            section.extend_from_slice(&u16::try_from(*length).unwrap().to_le_bytes());
            section.extend_from_slice(&u16::try_from(*base).unwrap().to_le_bytes());
        }
        while !section.len().is_multiple_of(4) {
            section.push(0);
        }
        for bit in &block_bits {
            section.extend_from_slice(&bit.to_le_bytes());
        }

        // Context map: every previous byte selects model 1.
        section.extend_from_slice(&[1u8; 256]);
        section.extend_from_slice(&2u16.to_le_bytes());
        let model = flat_model();
        let model_len = u16::try_from(model.len()).unwrap();
        section.extend_from_slice(&0u16.to_le_bytes());
        section.extend_from_slice(&model_len.to_le_bytes());
        section.extend_from_slice(&model);
        section.extend_from_slice(&model);
        section.extend_from_slice(&stream);
        section
    }

    /// Builds a global, checkpointed name section with flat eight-bit models.
    fn global_name_section(entries: &[(u32, String)]) -> Vec<u8> {
        const PAGE_COUNT: usize = 545;
        const BLOCK_SIZE: usize = 32;
        let block_count = entries.len().div_ceil(BLOCK_SIZE);

        let mut stream = Vec::new();
        let mut block_bits = Vec::with_capacity(block_count);
        for (id, (_, name)) in entries.iter().enumerate() {
            let previous = if id == 0 {
                ""
            } else {
                entries[id - 1].1.as_str()
            };
            if id.is_multiple_of(BLOCK_SIZE) {
                block_bits.push(u32::try_from(stream.len() * 8).unwrap());
                if id > 0 {
                    // The checkpoint is a full name under the dedicated order-0 model.
                    stream.extend_from_slice(previous.as_bytes());
                    stream.push(0);
                }
            }
            let prefix = previous
                .as_bytes()
                .iter()
                .zip(name.as_bytes())
                .take_while(|(left, right)| left == right)
                .count();
            let suffix = {
                let mut suffix = 0usize;
                while suffix < name.len() - prefix
                    && suffix < previous.len().saturating_sub(prefix)
                    && name.as_bytes()[name.len() - 1 - suffix]
                        == previous.as_bytes()[previous.len() - 1 - suffix]
                {
                    suffix += 1;
                }
                suffix
            };
            stream.push(u8::try_from(prefix).unwrap());
            stream.push(u8::try_from(suffix).unwrap());
            stream.extend_from_slice(&name.as_bytes()[prefix..name.len() - suffix]);
            stream.push(0);
        }

        let mut runs: Vec<(u32, u32, u32)> = Vec::new();
        let mut start = 0usize;
        for id in 1..=entries.len() {
            if id == entries.len() || entries[id].0 != entries[id - 1].0 + 1 {
                runs.push((
                    entries[start].0,
                    u32::try_from(id - start).unwrap(),
                    u32::try_from(start).unwrap(),
                ));
                start = id;
            }
        }

        let mut section = Vec::new();
        section.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
        section.extend_from_slice(&u32::try_from(block_count).unwrap().to_le_bytes());
        section.extend_from_slice(&u16::try_from(runs.len()).unwrap().to_le_bytes());
        section.extend_from_slice(&u16::try_from(PAGE_COUNT).unwrap().to_le_bytes());
        let mut directory = Vec::with_capacity(PAGE_COUNT);
        let mut index = 0usize;
        for page in 0..PAGE_COUNT {
            let base = u32::try_from(page).unwrap() << 11;
            while index < runs.len() && runs[index].0 + runs[index].1 <= base {
                index += 1;
            }
            directory.push(u16::try_from(index).unwrap());
        }
        for first in directory {
            section.extend_from_slice(&first.to_le_bytes());
        }
        for (first, length, base) in &runs {
            section.extend_from_slice(&first.to_le_bytes());
            section.extend_from_slice(&u16::try_from(*length).unwrap().to_le_bytes());
            section.extend_from_slice(&u16::try_from(*base).unwrap().to_le_bytes());
        }
        while !section.len().is_multiple_of(4) {
            section.push(0);
        }
        for bit in block_bits {
            section.extend_from_slice(&bit.to_le_bytes());
        }

        let model = flat_model();
        section.extend_from_slice(&[2u8; 256]);
        section.extend_from_slice(&5u16.to_le_bytes());
        for slot in 0..5u16 {
            section.extend_from_slice(&(slot * u16::try_from(model.len()).unwrap()).to_le_bytes());
        }
        for _ in 0..5 {
            section.extend_from_slice(&model);
        }
        section.extend_from_slice(&stream);
        section
    }

    /// Packs four sections into a Genesis image with the block-local name layout.
    fn pack_image(sections: [&[u8]; 4]) -> Vec<u8> {
        pack_image_layout(sections, 0)
    }

    /// Packs four sections into a Genesis image with an explicit name-layout byte.
    fn pack_image_layout(sections: [&[u8]; 4], layout: u8) -> Vec<u8> {
        let mut image = vec![0u8; HEADER_LEN];
        image[..4].copy_from_slice(MAGIC);
        image[4..6].copy_from_slice(&18u16.to_le_bytes());
        image[6..8].copy_from_slice(&0u16.to_le_bytes());
        image[8..10].copy_from_slice(&0u16.to_le_bytes());
        image[10] = 4;
        image[11] = 5;
        image[12] = 11;
        image[13] = layout;
        for (id, section) in sections.iter().enumerate() {
            while !image.len().is_multiple_of(8) {
                image.push(0);
            }
            let offset = u32::try_from(image.len()).unwrap();
            let length = u32::try_from(section.len()).unwrap();
            let at = 14 + id * 8;
            image[at..at + 4].copy_from_slice(&offset.to_le_bytes());
            image[at + 4..at + 8].copy_from_slice(&length.to_le_bytes());
            image.extend_from_slice(section);
        }
        image
    }

    /// The synthetic corpus: 70 names over seven runs, crossing three blocks.
    fn corpus() -> Vec<(u32, String)> {
        (0..70u32)
            .map(|index| {
                let cp = 0x100 + index + (index / 10) * 5;
                let name = match index % 3 {
                    0 => format!("TEST PREFIX {index:03} SUFFIX"),
                    1 => format!("TEST PREFIX {index:03} OTHER"),
                    _ => format!("OTHER NAME {index:03}"),
                };
                (cp, name)
            })
            .collect()
    }

    #[test]
    fn synthetic_image_resolves_every_name_in_any_order() {
        let entries = corpus();
        let patterns: &'static [u8] = pattern_section(&[
            (0x3400, 0x3401, "CJK UNIFIED IDEOGRAPH-", ""),
            (0x18CFF, 0x18CFF, "KHITAN SMALL SCRIPT CHARACTER-", ""),
        ])
        .leak();
        let names: &'static [u8] = name_section(&entries).leak();
        let image: &'static [u8] = pack_image([names, &[], &[], patterns]).leak();
        let mut genesis = Genesis::new(image).expect("a valid image");

        for (cp, name) in &entries {
            let mut out = String::new();
            assert!(genesis.write_name(*cp, &mut out), "U+{cp:04X}");
            assert_eq!(&out, name, "U+{cp:04X}");
            assert_eq!(genesis.name(*cp).as_deref(), Some(name.as_str()));
        }

        // Reverse order forces a fresh block decode for every entry; a pseudo-random walk
        // covers the remaining resume/reset combinations.
        for index in (0..entries.len()).rev() {
            let mut out = String::new();
            let (cp, name) = &entries[index];
            assert!(genesis.write_name(*cp, &mut out));
            assert_eq!(&out, name);
        }
        let mut state = 0x1234_5678u32;
        for _ in 0..300 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let index = usize::try_from(state % u32::try_from(entries.len()).unwrap()).unwrap();
            let mut out = String::new();
            let (cp, name) = &entries[index];
            assert!(genesis.write_name(*cp, &mut out));
            assert_eq!(&out, name);
        }

        // The gaps between the seven runs are misses.
        for cp in [0x10A, 0x11A, 0x164] {
            let mut out = String::from("unchanged");
            assert!(!genesis.write_name(cp, &mut out), "U+{cp:04X} must miss");
            assert_eq!(out, "unchanged");
        }
    }

    #[test]
    fn synthetic_global_image_resolves_every_name_in_any_order() {
        let entries = corpus();
        let names: &'static [u8] = global_name_section(&entries).leak();
        let image: &'static [u8] = pack_image_layout([names, &[], &[], &[]], 1).leak();
        let mut genesis = Genesis::new(image).expect("a valid global image");

        for (cp, name) in &entries {
            let mut out = String::new();
            assert!(genesis.write_name(*cp, &mut out), "U+{cp:04X}");
            assert_eq!(&out, name, "U+{cp:04X}");
        }
        for index in (0..entries.len()).rev() {
            let mut out = String::new();
            let (cp, name) = &entries[index];
            assert!(genesis.write_name(*cp, &mut out));
            assert_eq!(&out, name, "U+{cp:04X}");
        }
        // Cross-block sequential access must resume from the cursor, not reload checkpoints.
        let mut out = String::new();
        for (cp, name) in &entries {
            out.clear();
            assert!(genesis.write_name(*cp, &mut out));
            assert_eq!(&out, name);
        }
        for cp in [0x10A, 0x11A, 0x164] {
            let mut out = String::from("unchanged");
            assert!(!genesis.write_name(cp, &mut out), "U+{cp:04X} must miss");
            assert_eq!(out, "unchanged");
        }
    }

    #[test]
    fn synthetic_image_composes_hangul_patterns_and_labels() {
        let names: &'static [u8] = name_section(&corpus()).leak();
        let patterns: &'static [u8] = pattern_section(&[
            (0x3400, 0x3401, "CJK UNIFIED IDEOGRAPH-", ""),
            (0x18CFF, 0x18CFF, "KHITAN SMALL SCRIPT CHARACTER-", "-X"),
        ])
        .leak();
        let image: &'static [u8] = pack_image([names, &[], &[], patterns]).leak();
        let mut genesis = Genesis::new(image).expect("a valid image");

        let mut out = String::new();
        assert!(genesis.write_name(0xAC00, &mut out));
        assert_eq!(out, "HANGUL SYLLABLE GA");
        out.clear();
        assert!(genesis.write_name(0xD7A3, &mut out));
        assert_eq!(out, "HANGUL SYLLABLE HIH");
        out.clear();
        assert!(genesis.write_name(0x3401, &mut out));
        assert_eq!(out, "CJK UNIFIED IDEOGRAPH-3401");
        out.clear();
        assert!(genesis.write_name(0x18CFF, &mut out));
        assert_eq!(out, "KHITAN SMALL SCRIPT CHARACTER-18CFF-X");

        assert_eq!(genesis.name_or_label(0xF8FF), "<private-use-F8FF>");
        assert_eq!(genesis.name_or_label(0x378), "<reserved-0378>");
        assert_eq!(genesis.name_or_label(0xFFFE), "<noncharacter-FFFE>");
        assert_eq!(genesis.name_or_label(0x0), "<control-0000>");
        assert_eq!(
            genesis.name_or_label(0x100).as_ref(),
            "TEST PREFIX 000 SUFFIX"
        );
    }

    #[test]
    fn sections_are_returned_verbatim() {
        let names: &'static [u8] = name_section(&corpus()).leak();
        let patterns: &'static [u8] =
            pattern_section(&[(0x3400, 0x3401, "CJK UNIFIED IDEOGRAPH-", "")]).leak();
        let image: &'static [u8] = pack_image([names, b"emoji", b"gc", patterns]).leak();
        let genesis = Genesis::new(image).expect("a valid image");
        assert_eq!(genesis.section(NAMES), Some(names));
        assert_eq!(genesis.section(EMOJI), Some(b"emoji".as_slice()));
        assert_eq!(genesis.section(GC), Some(b"gc".as_slice()));
        assert_eq!(genesis.section(PATTERNS), Some(patterns));
        assert_eq!(genesis.section(4), None);
    }

    #[test]
    fn malformed_images_are_rejected() {
        assert!(Genesis::new(&[]).is_none());
        assert!(Genesis::new(b"nope").is_none());

        let names: &'static [u8] = name_section(&corpus()).leak();
        let image: &'static [u8] = pack_image([names, &[], &[], &[]]).leak();
        assert!(Genesis::new(image).is_some());

        let mut wrong_magic = image.to_vec();
        wrong_magic[0] = b'X';
        assert!(Genesis::new(wrong_magic.leak()).is_none());

        let mut wrong_count = image.to_vec();
        wrong_count[10] = 3;
        assert!(Genesis::new(wrong_count.leak()).is_none());

        let mut wrong_page = image.to_vec();
        wrong_page[12] = 10;
        assert!(Genesis::new(wrong_page.leak()).is_none());

        let mut out_of_bounds = image.to_vec();
        out_of_bounds[14..18].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Genesis::new(out_of_bounds.leak()).is_none());
    }
}
