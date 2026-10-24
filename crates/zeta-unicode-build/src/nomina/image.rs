//! Packing the flat Nomina image.
//!
//! ```text
//! off  size  field
//!   0     4  magic "NOM1"
//!   4     2  Unicode major
//!   6     2  Unicode minor
//!   8     2  Unicode patch
//!  10     1  section count (4)
//!  11     1  name block shift (5 -> 32 names per block)
//!  12     1  general-category page shift (11)
//!  13     1  name layout (0 block-local, 1 global checkpointed)
//!  14    32  4 x (payload offset u32, payload length u32)
//!  46    18  zero padding
//!  64    ..  section payloads, each starting on an 8-byte boundary
//! ```
//!
//! The directory starts at offset 14, exactly as the format contract states; the runtime
//! reads the `u32`s with unaligned little-endian loads, so no alignment padding is needed
//! before it. The header is padded to one 64-byte cache line, and only [`pack`] and the
//! runtime's `Nomina::new` ever read it.

/// The image magic.
pub const MAGIC: &[u8; 4] = b"NOM1";

/// The number of sections in a Nomina image.
pub const SECTION_COUNT: usize = 4;

/// The documented name-block shift: 32 names per block.
pub const NAME_BLOCK_SHIFT: u8 = 5;

/// The documented general-category page shift.
pub const GC_PAGE_SHIFT: u8 = 11;

/// The size of the padded image header.
pub const HEADER_LEN: usize = 64;

/// The section id of the stored character names.
pub const NAMES: usize = 0;
/// The section id of the emoji entries.
pub const EMOJI: usize = 1;
/// The section id of the general-category table.
pub const GC: usize = 2;
/// The section id of the `*` name patterns.
pub const PATTERNS: usize = 3;
/// The name layout byte for the block-local front-coding layout.
pub const LAYOUT_BLOCK_LOCAL: u8 = 0;
/// The name layout byte for the global checkpointed layout.
pub const LAYOUT_GLOBAL: u8 = 1;

/// Packs four sections into a flat Nomina image with the documented shifts.
///
/// Sections are appended in id order, each starting on an 8-byte boundary.
#[must_use]
pub fn pack(sections: [&[u8]; SECTION_COUNT], version: (u8, u8, u8)) -> Vec<u8> {
    pack_with(sections, version, NAME_BLOCK_SHIFT, GC_PAGE_SHIFT)
}

/// Packs four sections into a global-checkpointed Nomina image.
///
/// The name layout byte is set to [`LAYOUT_GLOBAL`]; the name section must have been built by
/// `nomina::names::encode_global`.
#[must_use]
pub fn pack_global(sections: [&[u8]; SECTION_COUNT], version: (u8, u8, u8)) -> Vec<u8> {
    pack_with_layout(
        sections,
        version,
        NAME_BLOCK_SHIFT,
        GC_PAGE_SHIFT,
        LAYOUT_GLOBAL,
    )
}

/// Packs four sections with explicit header shifts.
///
/// The shifts are exposed for the block-size and page-size measurements; the shipped image
/// uses [`pack`], so a decoder that ignores the header fields still sees the documented
/// layout.
///
/// # Panics
///
/// Panics when a section length or the final image size does not fit in a `u32`. The
/// generator never produces anything near that limit.
#[must_use]
pub fn pack_with(
    sections: [&[u8]; SECTION_COUNT],
    version: (u8, u8, u8),
    name_block_shift: u8,
    gc_page_shift: u8,
) -> Vec<u8> {
    pack_with_layout(
        sections,
        version,
        name_block_shift,
        gc_page_shift,
        LAYOUT_BLOCK_LOCAL,
    )
}

/// Packs four sections with explicit header shifts and a name-layout byte.
///
/// # Panics
///
/// Panics when a section length or the final image size does not fit in a `u32`, or when
/// `name_layout` is not one of [`LAYOUT_BLOCK_LOCAL`] / [`LAYOUT_GLOBAL`].
#[must_use]
pub fn pack_with_layout(
    sections: [&[u8]; SECTION_COUNT],
    version: (u8, u8, u8),
    name_block_shift: u8,
    gc_page_shift: u8,
    name_layout: u8,
) -> Vec<u8> {
    assert!(
        matches!(name_layout, LAYOUT_BLOCK_LOCAL | LAYOUT_GLOBAL),
        "unknown name layout {name_layout}"
    );
    let mut image = Vec::with_capacity(
        HEADER_LEN
            + sections
                .iter()
                .map(|section| section.len() + 7)
                .sum::<usize>(),
    );
    image.resize(HEADER_LEN, 0);
    image[..4].copy_from_slice(MAGIC);
    image[4..6].copy_from_slice(&u16::from(version.0).to_le_bytes());
    image[6..8].copy_from_slice(&u16::from(version.1).to_le_bytes());
    image[8..10].copy_from_slice(&u16::from(version.2).to_le_bytes());
    image[10] = u8::try_from(SECTION_COUNT).expect("four sections fit in a byte");
    image[11] = name_block_shift;
    image[12] = gc_page_shift;
    image[13] = name_layout;

    for (id, section) in sections.iter().enumerate() {
        while image.len() % 8 != 0 {
            image.push(0);
        }
        let offset = u32::try_from(image.len()).expect("image offset fits in u32");
        let length = u32::try_from(section.len()).expect("section length fits in u32");
        let directory = 14 + id * 8;
        image[directory..directory + 4].copy_from_slice(&offset.to_le_bytes());
        image[directory + 4..directory + 8].copy_from_slice(&length.to_le_bytes());
        image.extend_from_slice(section);
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads a little-endian `u32` at `at`.
    fn u32_at(buf: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(buf[at..at + 4].try_into().expect("four bytes"))
    }

    #[test]
    fn packs_the_documented_header_and_aligned_sections() {
        let sections: [&[u8]; SECTION_COUNT] = [b"names", b"", b"gc", b"patterns"];
        let image = pack(sections, (18, 0, 0));

        assert_eq!(&image[..4], MAGIC);
        assert_eq!(u16::from_le_bytes([image[4], image[5]]), 18);
        assert_eq!(u16::from_le_bytes([image[6], image[7]]), 0);
        assert_eq!(u16::from_le_bytes([image[8], image[9]]), 0);
        assert_eq!(image[10], 4);
        assert_eq!(image[11], 5);
        assert_eq!(image[12], 11);
        assert_eq!(image[13], 0);
        assert_eq!(image.len() % 8, 0);
        assert_eq!(&image[46..64], &[0; 18]);

        let mut previous_end = HEADER_LEN;
        for (id, section) in sections.iter().enumerate() {
            let directory = 14 + id * 8;
            let offset = u32_at(&image, directory) as usize;
            let length = u32_at(&image, directory + 4) as usize;
            assert_eq!(length, section.len());
            assert_eq!(offset % 8, 0, "section {id} is not aligned");
            assert!(
                offset >= previous_end,
                "section {id} overlaps its predecessor"
            );
            assert_eq!(&image[offset..offset + length], *section);
            previous_end = offset + length;
        }
    }

    #[test]
    fn explicit_shifts_are_recorded() {
        let image = pack_with([b"", b"", b"", b""], (1, 2, 3), 7, 12);
        assert_eq!(image[11], 7);
        assert_eq!(image[12], 12);
        assert_eq!(image[13], LAYOUT_BLOCK_LOCAL);
        assert_eq!(u16::from_le_bytes([image[4], image[5]]), 1);
        assert_eq!(u16::from_le_bytes([image[6], image[7]]), 2);
        assert_eq!(u16::from_le_bytes([image[8], image[9]]), 3);
    }

    #[test]
    fn the_global_pack_sets_the_layout_byte() {
        let image = pack_global([b"", b"", b"", b""], (18, 0, 0));
        assert_eq!(image[13], LAYOUT_GLOBAL);
        let image = pack_with_layout([b"", b"", b"", b""], (18, 0, 0), 5, 11, LAYOUT_GLOBAL);
        assert_eq!(image[13], LAYOUT_GLOBAL);
    }

    /// Builds the complete measurement image and writes it to `NOMINA_IMAGE` (default
    /// `$TMPDIR/nomina.bin`). The runtime benchmark loads it read-only.
    #[test]
    #[ignore = "writes the measurement image"]
    fn writes_the_measurement_image() {
        use std::path::Path;

        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../zeta-unicode/data");
        let inputs = crate::ucd::read_inputs(&data).expect("vendored inputs");
        let block_shift = std::env::var("NOMINA_BLOCK_SHIFT")
            .ok()
            .map_or(crate::nomina::names::NAME_BLOCK_SHIFT, |value| {
                value.parse().expect("a numeric block shift")
            });
        let global = std::env::var("NOMINA_LAYOUT").is_ok_and(|value| value == "global");
        let encoded = if global {
            crate::nomina::names::encode_global_with_block_shift(&inputs, block_shift)
        } else {
            crate::nomina::names::encode_with_block_shift(&inputs, block_shift)
        };
        let names = encoded.bytes;
        let (emoji, emoji_stats) =
            crate::nomina::emoji::build(&inputs.emoji, &inputs.sequences, &inputs.names);
        let (gc, gc_stats) = crate::nomina::gc::build(&inputs.gc);
        let (patterns, pattern_stats) = crate::nomina::patterns::build(&inputs.names);
        let image = pack_with_layout(
            [&names, &emoji, &gc, &patterns],
            inputs.version,
            block_shift,
            GC_PAGE_SHIFT,
            encoded.layout.id(),
        );
        let path = std::env::var_os("NOMINA_IMAGE").map_or_else(
            || std::env::temp_dir().join("nomina.bin"),
            std::path::PathBuf::from,
        );
        std::fs::write(&path, &image).expect("write the measurement image");

        eprintln!(
            "image {} B: names {} + emoji {} + gc {} + patterns {} (+ header/padding {})",
            image.len(),
            names.len(),
            emoji.len(),
            gc.len(),
            patterns.len(),
            image.len() - names.len() - emoji.len() - gc.len() - patterns.len()
        );
        eprintln!(
            "emoji: {} entries, {} heap; gc: {} runs, {} heap; patterns: {} ranges",
            emoji_stats.entries,
            emoji_stats.heap_bytes,
            gc_stats.runs,
            gc_stats.heap_bytes,
            pattern_stats.patterns
        );
        eprintln!("wrote {}", path.display());
    }
}
