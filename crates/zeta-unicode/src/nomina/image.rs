//! The flat Nomina image and its four sections.
//!
//! [`Nomina::new`] validates the 64-byte header, the name section's index and the pattern table
//! once; every lookup then reads the image slices directly. There is no startup decode and no
//! `OnceLock` in [`Nomina`]; [`section`] reads one section without building the name decoder.
//!
//! Byte 13 of the header selects the name layout: 0 is the block-local front-coding layout,
//! 1 is the global checkpointed layout. Both are decoded by [`super::names`]; the runtime
//! accepts either.
//!
//! ```text
//! off  field
//!   0  magic "NOM1"
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
//! Section 3 is the `*`-pattern table written by `zeta_unicode_build::nomina::patterns`:
//! a pattern count, a prefix dictionary, a suffix dictionary, then one record per range
//! (`varint start delta`, `varint length`, `u8 prefix id`, `u8 suffix id`). Pattern names are
//! `prefix + upper-case hex(cp) + suffix`, with the hex padded to at least four digits so a
//! future range below U+1000 formats the way the trainer's verifier writes it.

use zeta_unicode_format::{Header, UNICODE_END, read_varint, write_pattern_name};

use super::names::{self, Names};

pub use zeta_unicode_format::{EMOJI, GC, NAMES, PATTERNS};

/// Returns the payload of section `id` from a Nomina image.
///
/// This validates only the header fields needed to locate a section, so a caller that needs one
/// section does not build the name decoder. Returns `None` for a foreign magic, a different
/// section count, an unknown id or an out-of-bounds range.
#[must_use]
pub fn section(table: &[u8], id: usize) -> Option<&[u8]> {
    Header::parse(table)?.section(table, id)
}

/// A parsed Nomina image.
pub struct Nomina<'a> {
    /// The decoded name section.
    names: Names<'a>,
    /// The `*`-pattern section.
    patterns: Patterns<'a>,
}

impl<'a> Nomina<'a> {
    /// Parses a Nomina image.
    ///
    /// Returns `None` when the magic, section count, header shifts, section bounds, the name
    /// section or the pattern section are invalid. The name section's index is validated too:
    /// runs must be ordered, disjoint and in range, their base ids must tile the name ids, and
    /// the page directory must be a non-decreasing list of run indices.
    ///
    /// The image must outlive the returned [`Nomina`] because the decoder reads it in place.
    #[must_use]
    pub fn new(table: &'a [u8]) -> Option<Self> {
        let header = Header::parse(table)?;
        let names = Names::new(
            header.section(table, NAMES)?,
            header.block_shift,
            header.layout,
        )?;
        let patterns = Patterns::new(header.section(table, PATTERNS)?)?;
        Some(Self { names, patterns })
    }

    /// Appends the name of `cp` to `out`, returning `false` when it has no name.
    ///
    /// This is the zero-allocation path: the only buffers are the name section's fixed 256-byte
    /// scratch arrays. A stored name decodes at most one block.
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
    /// Stored names, Hangul names and pattern names all come back owned because they are
    /// decoded into a temporary string. Use [`Self::write_name`] to avoid the allocation.
    #[must_use]
    pub fn name(&mut self, cp: u32) -> Option<String> {
        let mut out = String::new();
        self.write_name(cp, &mut out).then_some(out)
    }
}

/// The `*`-pattern section.
struct Patterns<'a> {
    /// The validated ranges, sorted by start code point and non-overlapping.
    ranges: Vec<Pattern<'a>>,
}

/// One validated `*`-pattern range.
struct Pattern<'a> {
    /// First code point in the range.
    start: u32,
    /// Last code point in the range, inclusive.
    end: u32,
    /// Text before the `*` placeholder.
    prefix: &'a str,
    /// Text after the `*` placeholder.
    suffix: &'a str,
}

impl<'a> Patterns<'a> {
    /// Parses a pattern section; an empty section is valid and matches nothing.
    fn new(section: &'a [u8]) -> Option<Self> {
        if section.is_empty() {
            return Some(Self { ranges: Vec::new() });
        }
        let count = usize::from(*section.first()?);
        let (prefixes, at) = read_dictionary(section, 1)?;
        let (suffixes, at) = read_dictionary(section, at)?;
        let mut records = section.get(at..)?;
        let mut ranges = Vec::with_capacity(count);
        let mut previous_end = None;
        let mut previous_start = 0u32;
        for _ in 0..count {
            let (delta, used) = read_varint(records)?;
            records = records.get(used..)?;
            let (length, used) = read_varint(records)?;
            records = records.get(used..)?;
            let prefix = *prefixes.get(usize::from(*records.first()?))?;
            let suffix = *suffixes.get(usize::from(*records.get(1)?))?;
            records = records.get(2..)?;

            let start = previous_start.checked_add(delta)?;
            let length = length.checked_sub(1)?;
            let end = start.checked_add(length)?;
            if start > UNICODE_END
                || end > UNICODE_END
                || previous_end.is_some_and(|previous| start <= previous)
            {
                return None;
            }
            ranges.push(Pattern {
                start,
                end,
                prefix,
                suffix,
            });
            previous_start = start;
            previous_end = Some(end);
        }
        if !records.is_empty() {
            return None;
        }
        Some(Self { ranges })
    }

    /// Appends the pattern name of `cp` to `out`, returning `false` when no range covers it.
    fn write(&self, cp: u32, out: &mut String) -> bool {
        let index = self.ranges.partition_point(|range| range.end < cp);
        let Some(pattern) = self.ranges.get(index) else {
            return false;
        };
        if cp < pattern.start {
            return false;
        }
        write_pattern_name(pattern.prefix, pattern.suffix, cp, out);
        true
    }
}

/// Reads a `u8`-counted dictionary of `u8`-length UTF-8 strings, returning it and the next
/// offset. A dictionary entry that is not valid UTF-8 rejects the section, so a lookup can
/// push the strings it returns without a partial write.
fn read_dictionary(section: &[u8], at: usize) -> Option<(Vec<&str>, usize)> {
    let count = usize::from(*section.get(at)?);
    let mut cursor = at + 1;
    let mut dictionary = Vec::with_capacity(count);
    for _ in 0..count {
        let length = usize::from(*section.get(cursor)?);
        cursor += 1;
        let text = std::str::from_utf8(section.get(cursor..cursor + length)?).ok()?;
        cursor += length;
        dictionary.push(text);
    }
    Some((dictionary, cursor))
}

#[cfg(test)]
mod tests {
    use zeta_unicode_format::{
        DIRECTORY_AT, DIRECTORY_ENTRY_LEN, GC_PAGE_SHIFT, GC_PAGE_SHIFT_AT, HEADER_LEN, MAGIC,
        NAME_BLOCK_SHIFT_AT, NAME_LAYOUT_AT, NAME_PAGE_COUNT, NAME_PAGE_SHIFT, SECTION_COUNT,
        SECTION_COUNT_AT, VERSION_AT, write_varint,
    };

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
            write_varint(lo - previous, &mut section);
            write_varint(hi - lo + 1, &mut section);
            section
                .push(u8::try_from(prefixes.iter().position(|p| *p == prefix).unwrap()).unwrap());
            section
                .push(u8::try_from(suffixes.iter().position(|s| *s == suffix).unwrap()).unwrap());
            previous = lo;
        }
        section
    }

    #[test]
    fn patterns_resolve_prefix_hex_and_suffix() {
        let section = pattern_section(&[
            (0x3400, 0x3401, "CJK UNIFIED IDEOGRAPH-", ""),
            (0x18CFF, 0x18CFF, "KHITAN SMALL SCRIPT CHARACTER-", "-X"),
        ]);
        let patterns = Patterns::new(&section).expect("a valid pattern section");

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
        let section = pattern_section(&[(0x3400, 0x3401, "CJK UNIFIED IDEOGRAPH-", "")]);
        assert!(Patterns::new(&section).is_some());
        assert!(Patterns::new(&section[..section.len() - 1]).is_none());
    }

    #[test]
    fn every_truncation_of_a_pattern_section_is_rejected() {
        let section = pattern_section(&[(0x41, 0x41, "A-", "")]);
        assert!(Patterns::new(&section).is_some());
        // An empty section is a valid, empty table rather than a truncated one.
        assert!(Patterns::new(&section[..0]).is_some());
        for length in 1..section.len() {
            assert!(
                Patterns::new(&section[..length]).is_none(),
                "length {length}"
            );
        }
    }

    #[test]
    fn pattern_hex_is_padded_to_four_digits() {
        let section = pattern_section(&[
            (0x10, 0x12, "TEST-", ""),
            (0x18CFF, 0x18CFF, "LONGER-", "-END"),
        ]);
        let patterns = Patterns::new(&section).expect("a valid pattern section");

        let mut out = String::new();
        assert!(patterns.write(0x10, &mut out));
        assert_eq!(out, "TEST-0010", "the usual four-digit placeholder");
        out.clear();
        assert!(patterns.write(0x12, &mut out));
        assert_eq!(out, "TEST-0012");
        out.clear();
        assert!(patterns.write(0x18CFF, &mut out));
        assert_eq!(
            out, "LONGER-18CFF-END",
            "five digits where four are not enough"
        );
        out.clear();
        assert!(!patterns.write(0xF, &mut out));
        assert!(!patterns.write(0x13, &mut out));
        assert!(!patterns.write(0x18D00, &mut out));
        assert_eq!(out, "", "a miss must not extend the output");
    }

    #[test]
    fn a_pattern_section_with_non_utf8_dictionaries_is_rejected() {
        // One pattern whose prefix dictionary entry is not valid UTF-8.
        let prefix_case = vec![1, 1, 2, 0xFF, 0xFE, 0, 0x41, 1, 0, 0];
        assert!(Patterns::new(&prefix_case).is_none());

        // The suffix dictionary is validated the same way.
        let suffix_case = vec![1, 1, 1, b'A', 1, 2, 0xFF, 0xFE, 0x41, 1, 0, 0];
        assert!(Patterns::new(&suffix_case).is_none());
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

    /// The maximal runs of consecutive code points in `entries`.
    fn runs(entries: &[(u32, String)]) -> Vec<(u32, u32, u32)> {
        let mut runs = Vec::new();
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
        runs
    }

    /// Appends the header, page directory and run records shared by both layouts.
    fn push_index(
        section: &mut Vec<u8>,
        entries: &[(u32, String)],
        block_count: usize,
        runs: &[(u32, u32, u32)],
    ) {
        section.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
        section.extend_from_slice(&u32::try_from(block_count).unwrap().to_le_bytes());
        section.extend_from_slice(&u16::try_from(runs.len()).unwrap().to_le_bytes());
        section.extend_from_slice(&u16::try_from(NAME_PAGE_COUNT).unwrap().to_le_bytes());
        let mut index = 0usize;
        for page in 0..NAME_PAGE_COUNT {
            let base = u32::try_from(page).unwrap() << NAME_PAGE_SHIFT;
            while index < runs.len() && runs[index].0 + runs[index].1 <= base {
                index += 1;
            }
            section.extend_from_slice(&u16::try_from(index).unwrap().to_le_bytes());
        }
        for &(first, length, base) in runs {
            section.extend_from_slice(&first.to_le_bytes());
            section.extend_from_slice(&u16::try_from(length).unwrap().to_le_bytes());
            section.extend_from_slice(&u16::try_from(base).unwrap().to_le_bytes());
        }
        while !section.len().is_multiple_of(4) {
            section.push(0);
        }
    }

    /// Appends a flat model area: `count` copies of `model`, each symbol eight bits wide.
    fn push_flat_models(section: &mut Vec<u8>, context: u8, count: u16, model: &[u8]) {
        section.extend_from_slice(&[context; 256]);
        section.extend_from_slice(&count.to_le_bytes());
        for slot in 0..count {
            section.extend_from_slice(&(slot * u16::try_from(model.len()).unwrap()).to_le_bytes());
        }
        for _ in 0..count {
            section.extend_from_slice(model);
        }
    }

    /// Builds a name section whose bitstream is exactly `prefix, suffix bytes, 0` per entry.
    fn name_section(entries: &[(u32, String)]) -> Vec<u8> {
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

        let runs = runs(entries);
        let mut section = Vec::new();
        push_index(&mut section, entries, block_count, &runs);
        for bit in &block_bits {
            section.extend_from_slice(&bit.to_le_bytes());
        }
        let model = flat_model();
        push_flat_models(&mut section, 1, 2, &model);
        section.extend_from_slice(&stream);
        section
    }

    /// Builds a global, checkpointed name section with flat eight-bit models.
    fn global_name_section(entries: &[(u32, String)]) -> Vec<u8> {
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

        let runs = runs(entries);
        let mut section = Vec::new();
        push_index(&mut section, entries, block_count, &runs);
        for bit in block_bits {
            section.extend_from_slice(&bit.to_le_bytes());
        }
        let model = flat_model();
        push_flat_models(&mut section, 2, 5, &model);
        section.extend_from_slice(&stream);
        section
    }

    /// Packs four sections into a Nomina image with the block-local name layout.
    fn pack_image(sections: [&[u8]; 4]) -> Vec<u8> {
        pack_image_layout(sections, 0)
    }

    /// Packs four sections into a Nomina image with an explicit name-layout byte.
    fn pack_image_layout(sections: [&[u8]; 4], layout: u8) -> Vec<u8> {
        let mut image = vec![0u8; HEADER_LEN];
        image[..4].copy_from_slice(MAGIC);
        image[VERSION_AT..VERSION_AT + 2].copy_from_slice(&18u16.to_le_bytes());
        image[VERSION_AT + 2..VERSION_AT + 4].copy_from_slice(&0u16.to_le_bytes());
        image[VERSION_AT + 4..VERSION_AT + 6].copy_from_slice(&0u16.to_le_bytes());
        image[SECTION_COUNT_AT] = u8::try_from(SECTION_COUNT).unwrap();
        image[NAME_BLOCK_SHIFT_AT] = 5;
        image[GC_PAGE_SHIFT_AT] = GC_PAGE_SHIFT;
        image[NAME_LAYOUT_AT] = layout;
        for (id, section) in sections.iter().enumerate() {
            while !image.len().is_multiple_of(8) {
                image.push(0);
            }
            let offset = u32::try_from(image.len()).unwrap();
            let length = u32::try_from(section.len()).unwrap();
            let at = DIRECTORY_AT + id * DIRECTORY_ENTRY_LEN;
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
        let patterns = pattern_section(&[
            (0x3400, 0x3401, "CJK UNIFIED IDEOGRAPH-", ""),
            (0x18CFF, 0x18CFF, "KHITAN SMALL SCRIPT CHARACTER-", ""),
        ]);
        let names = name_section(&entries);
        let image = pack_image([&names, &[], &[], &patterns]);
        let mut nomina = Nomina::new(&image).expect("a valid image");

        for (cp, name) in &entries {
            let mut out = String::new();
            assert!(nomina.write_name(*cp, &mut out), "U+{cp:04X}");
            assert_eq!(&out, name, "U+{cp:04X}");
            assert_eq!(nomina.name(*cp).as_deref(), Some(name.as_str()));
        }

        // Reverse order forces a fresh block decode for every entry; a pseudo-random walk
        // covers the remaining resume/reset combinations.
        for index in (0..entries.len()).rev() {
            let mut out = String::new();
            let (cp, name) = &entries[index];
            assert!(nomina.write_name(*cp, &mut out));
            assert_eq!(&out, name);
        }
        let mut state = 0x1234_5678u32;
        for _ in 0..300 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let index = usize::try_from(state % u32::try_from(entries.len()).unwrap()).unwrap();
            let mut out = String::new();
            let (cp, name) = &entries[index];
            assert!(nomina.write_name(*cp, &mut out));
            assert_eq!(&out, name);
        }

        // The gaps between the seven runs are misses.
        for cp in [0x10A, 0x11A, 0x164] {
            let mut out = String::from("unchanged");
            assert!(!nomina.write_name(cp, &mut out), "U+{cp:04X} must miss");
            assert_eq!(out, "unchanged");
        }
    }

    #[test]
    fn synthetic_global_image_resolves_every_name_in_any_order() {
        let entries = corpus();
        let names = global_name_section(&entries);
        let image = pack_image_layout([&names, &[], &[], &[]], 1);
        let mut nomina = Nomina::new(&image).expect("a valid global image");

        for (cp, name) in &entries {
            let mut out = String::new();
            assert!(nomina.write_name(*cp, &mut out), "U+{cp:04X}");
            assert_eq!(&out, name, "U+{cp:04X}");
        }
        for index in (0..entries.len()).rev() {
            let mut out = String::new();
            let (cp, name) = &entries[index];
            assert!(nomina.write_name(*cp, &mut out));
            assert_eq!(&out, name, "U+{cp:04X}");
        }
        // Cross-block sequential access must resume from the cursor, not reload checkpoints.
        let mut out = String::new();
        for (cp, name) in &entries {
            out.clear();
            assert!(nomina.write_name(*cp, &mut out));
            assert_eq!(&out, name);
        }
        for cp in [0x10A, 0x11A, 0x164] {
            let mut out = String::from("unchanged");
            assert!(!nomina.write_name(cp, &mut out), "U+{cp:04X} must miss");
            assert_eq!(out, "unchanged");
        }
    }

    #[test]
    fn synthetic_image_composes_hangul_and_patterns() {
        let names = name_section(&corpus());
        let patterns = pattern_section(&[
            (0x3400, 0x3401, "CJK UNIFIED IDEOGRAPH-", ""),
            (0x18CFF, 0x18CFF, "KHITAN SMALL SCRIPT CHARACTER-", "-X"),
        ]);
        let image = pack_image([&names, &[], &[], &patterns]);
        let mut nomina = Nomina::new(&image).expect("a valid image");

        let mut out = String::new();
        assert!(nomina.write_name(0xAC00, &mut out));
        assert_eq!(out, "HANGUL SYLLABLE GA");
        out.clear();
        assert!(nomina.write_name(0xD7A3, &mut out));
        assert_eq!(out, "HANGUL SYLLABLE HIH");
        out.clear();
        assert!(nomina.write_name(0x3401, &mut out));
        assert_eq!(out, "CJK UNIFIED IDEOGRAPH-3401");
        out.clear();
        assert!(nomina.write_name(0x18CFF, &mut out));
        assert_eq!(out, "KHITAN SMALL SCRIPT CHARACTER-18CFF-X");
    }

    #[test]
    fn sections_are_returned_verbatim() {
        let names = name_section(&corpus());
        let patterns = pattern_section(&[(0x3400, 0x3401, "CJK UNIFIED IDEOGRAPH-", "")]);
        let image = pack_image([&names, b"emoji", b"gc", &patterns]);
        assert_eq!(section(&image, NAMES), Some(names.as_slice()));
        assert_eq!(section(&image, EMOJI), Some(b"emoji".as_slice()));
        assert_eq!(section(&image, GC), Some(b"gc".as_slice()));
        assert_eq!(section(&image, PATTERNS), Some(patterns.as_slice()));
        assert_eq!(section(&image, SECTION_COUNT), None);
        assert_eq!(section(b"nope", NAMES), None);
    }

    #[test]
    fn malformed_images_are_rejected() {
        assert!(Nomina::new(&[]).is_none());
        assert!(Nomina::new(b"nope").is_none());

        let names = name_section(&corpus());
        let image = pack_image([&names, &[], &[], &[]]);
        assert!(Nomina::new(&image).is_some());

        let mut wrong_magic = image.clone();
        wrong_magic[0] = b'X';
        assert!(Nomina::new(&wrong_magic).is_none());

        let mut wrong_count = image.clone();
        wrong_count[SECTION_COUNT_AT] = 3;
        assert!(Nomina::new(&wrong_count).is_none());

        let mut wrong_page = image.clone();
        wrong_page[GC_PAGE_SHIFT_AT] = 10;
        assert!(Nomina::new(&wrong_page).is_none());

        let mut wrong_shift = image.clone();
        wrong_shift[NAME_BLOCK_SHIFT_AT] = 0;
        assert!(Nomina::new(&wrong_shift).is_none());
        let mut wrong_shift = image.clone();
        wrong_shift[NAME_BLOCK_SHIFT_AT] = 9;
        assert!(Nomina::new(&wrong_shift).is_none());

        let mut wrong_layout = image.clone();
        wrong_layout[NAME_LAYOUT_AT] = 2;
        assert!(Nomina::new(&wrong_layout).is_none());

        let mut out_of_bounds = image;
        out_of_bounds[DIRECTORY_AT..DIRECTORY_AT + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Nomina::new(&out_of_bounds).is_none());
    }
    #[test]
    fn every_truncation_of_an_image_is_rejected() {
        let names = name_section(&corpus());
        let image = pack_image([&names, &[], &[], &[]]);
        assert!(Nomina::new(&image).is_some());
        for length in 0..image.len() {
            assert!(Nomina::new(&image[..length]).is_none(), "length {length}");
        }
    }
}
