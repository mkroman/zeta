//! The Nomina wire layout: image header, section ids, name and category tables.
//!
//! These constants are the contract between `zeta_unicode_build::nomina` and the runtime
//! decoders in `zeta_unicode`. Both crates import them from here instead of spelling the same
//! offsets and shifts twice, so a format change is one edit rather than two that can drift.

/// The image magic.
pub const MAGIC: &[u8; 4] = b"NOM1";

/// The number of sections in a Nomina image.
pub const SECTION_COUNT: usize = 4;

/// The size of the padded image header.
pub const HEADER_LEN: usize = 64;

/// The offset of the Unicode version fields: three little-endian `u16`s.
pub const VERSION_AT: usize = 4;

/// The offset of the section-count byte.
pub const SECTION_COUNT_AT: usize = 10;

/// The offset of the name-block-shift byte.
pub const NAME_BLOCK_SHIFT_AT: usize = 11;

/// The offset of the general-category page-shift byte.
pub const GC_PAGE_SHIFT_AT: usize = 12;

/// The offset of the name-layout byte.
pub const NAME_LAYOUT_AT: usize = 13;

/// The offset of the four `(offset, length)` section-directory entries.
pub const DIRECTORY_AT: usize = 14;

/// The size of one `(offset, length)` directory entry.
pub const DIRECTORY_ENTRY_LEN: usize = 8;

/// The section id of the stored character names.
pub const NAMES: usize = 0;
/// The section id of the emoji entries.
pub const EMOJI: usize = 1;
/// The section id of the general category.
pub const GC: usize = 2;
/// The section id of the `*` patterns.
pub const PATTERNS: usize = 3;

/// Byte 13 of the header selects how the name section front-codes its entries.
///
/// The discriminants are the wire ids written by the trainer, so they must never change.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameLayout {
    /// The predecessor resets at every block boundary.
    BlockLocal = 0,
    /// Every name uses its code-point predecessor; each block starts with a checkpoint.
    Global = 1,
}

impl NameLayout {
    /// The layout id written into the image header.
    #[must_use]
    pub const fn id(self) -> u8 {
        self as u8
    }

    /// Parses a layout id from the image header.
    #[must_use]
    pub const fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::BlockLocal),
            1 => Some(Self::Global),
            _ => None,
        }
    }

    /// Is this the global checkpointed layout?
    #[must_use]
    pub const fn is_global(self) -> bool {
        matches!(self, Self::Global)
    }
}

/// The parsed 64-byte image header and its section directory.
///
/// Both the runtime and the trainer use this one parse, so the magic, version, shifts, layout
/// byte and section bounds cannot drift between them. The trainer additionally enforces that
/// section payloads are aligned, ordered and exhaust the image; the runtime only needs the
/// ranges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// The Unicode major, minor and patch version.
    pub version: (u8, u8, u8),
    /// The name-block shift: `1 << block_shift` names per front-coded block.
    pub block_shift: u8,
    /// The general-category page shift.
    pub gc_page_shift: u8,
    /// The name-section layout.
    pub layout: NameLayout,
    /// `(offset, length)` of each section, indexed by section id.
    pub sections: [(usize, usize); SECTION_COUNT],
}

impl Header {
    /// Parses and validates the image header and section directory.
    ///
    /// Returns `None` for a truncated header, a foreign magic, a different section count, an
    /// unsupported version field, shift or layout id, or a section range outside `image`.
    #[must_use]
    pub fn parse(image: &[u8]) -> Option<Self> {
        if image.len() < HEADER_LEN || image.get(..MAGIC.len()) != Some(MAGIC.as_slice()) {
            return None;
        }
        if usize::from(*image.get(SECTION_COUNT_AT)?) != SECTION_COUNT {
            return None;
        }
        let version = (
            u8::try_from(read_u16(image, VERSION_AT)?).ok()?,
            u8::try_from(read_u16(image, VERSION_AT + 2)?).ok()?,
            u8::try_from(read_u16(image, VERSION_AT + 4)?).ok()?,
        );
        let block_shift = *image.get(NAME_BLOCK_SHIFT_AT)?;
        if !(MIN_NAME_BLOCK_SHIFT..=MAX_NAME_BLOCK_SHIFT).contains(&block_shift) {
            return None;
        }
        let gc_page_shift = *image.get(GC_PAGE_SHIFT_AT)?;
        if gc_page_shift != GC_PAGE_SHIFT {
            return None;
        }
        let layout = NameLayout::from_id(*image.get(NAME_LAYOUT_AT)?)?;

        let mut sections = [(0usize, 0usize); SECTION_COUNT];
        for (id, slot) in sections.iter_mut().enumerate() {
            let at = DIRECTORY_AT.checked_add(id.checked_mul(DIRECTORY_ENTRY_LEN)?)?;
            let offset = usize::try_from(read_u32(image, at)?).ok()?;
            let length = usize::try_from(read_u32(image, at.checked_add(4)?)?).ok()?;
            if offset.checked_add(length)? > image.len() {
                return None;
            }
            *slot = (offset, length);
        }
        Some(Self {
            version,
            block_shift,
            gc_page_shift,
            layout,
            sections,
        })
    }

    /// Returns the payload of section `id`, or `None` for an unknown id or a truncated range.
    #[must_use]
    pub fn section<'a>(&self, image: &'a [u8], id: usize) -> Option<&'a [u8]> {
        let &(offset, length) = self.sections.get(id)?;
        image.get(offset..offset.checked_add(length)?)
    }
}

/// Reads a little-endian `u16` at `at`.
fn read_u16(image: &[u8], at: usize) -> Option<u16> {
    let bytes: [u8; 2] = image.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_le_bytes(bytes))
}

/// Reads a little-endian `u32` at `at`.
fn read_u32(image: &[u8], at: usize) -> Option<u32> {
    let bytes: [u8; 4] = image.get(at..at.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

/// The first Unicode scalar value.
pub const UNICODE_START: u32 = 0;
/// The last Unicode scalar value.
pub const UNICODE_END: u32 = 0x10_FFFF;
/// The number of code points in the Unicode range, including surrogates.
pub const UNICODE_LIMIT: u32 = 0x11_0000;

/// The code-point page shift of the general-category runs.
pub const GC_SHIFT: u32 = 11;

/// The general-category page shift stored in the image header.
pub const GC_PAGE_SHIFT: u8 = 11;

const _: () = assert!(GC_PAGE_SHIFT as u32 == GC_SHIFT);

/// The number of pages in the general-category table.
///
/// This is one more than [`UNICODE_END`] needs, so the runtime's page directory carries a
/// final sentinel entry and the run search on the last page has an upper bound.
pub const GC_PAGES: usize = (UNICODE_LIMIT as usize >> GC_SHIFT) + 1;

/// The low bits of a general-category record holding the category id.
pub const GC_ID_BITS: u32 = 5;

/// The mask that extracts the category id from a general-category record.
pub const GC_ID_MASK: u16 = (1u16 << GC_ID_BITS) - 1;

/// The general category abbreviations in table id order (id = position + 1).
///
/// The generator writes these ids and the runtime indexes this array, so both must use the same
/// order.
pub const GC_SHORT_NAMES: [&str; 30] = [
    "Cc", "Cf", "Cn", "Co", "Cs", "Ll", "Lm", "Lo", "Lt", "Lu", "Mc", "Me", "Mn", "Nd", "Nl", "No",
    "Pc", "Pd", "Pe", "Pf", "Pi", "Po", "Ps", "Sc", "Sk", "Sm", "So", "Zl", "Zp", "Zs",
];

/// The code-point page shift of the name-run directory.
pub const NAME_PAGE_SHIFT: u32 = GC_SHIFT;

/// The number of pages in the name-run directory.
///
/// The name and general-category tables both page the code space in 2,048-code-point steps and
/// both keep a final sentinel page.
pub const NAME_PAGE_COUNT: usize = GC_PAGES;

/// The largest name the runtime's fixed scratch buffer holds, in bytes.
pub const MAX_NAME_BYTES: usize = 256;

/// Bytes in the name section's fixed index header: two `u32` counts and two `u16` counts.
pub const NAME_INDEX_HEADER_LEN: usize = 4 + 4 + 2 + 2;

/// Bytes per name page-directory entry (`u16` first-run index).
pub const NAME_PAGE_DIRECTORY_ENTRY_LEN: usize = 2;

/// Bytes per name run record: `u32 first_cp`, `u16 length`, `u16 base_id`.
pub const NAME_RUN_LEN: usize = 8;

/// Bytes of the model-area context map: one model id per possible previous byte.
pub const NAME_CONTEXT_MAP_LEN: usize = 256;

/// Bytes of the model-area model-count field.
pub const NAME_MODEL_COUNT_LEN: usize = 2;

/// Bytes per model-directory entry.
pub const NAME_MODEL_DIRECTORY_ENTRY_LEN: usize = 2;

/// The longest canonical-Huffman code a name model may use, in bits.
pub const MAX_CODE_BITS: u8 = 16;

/// The most models a name section may declare: the prefix, byte-context, generic, suffix and
/// checkpoint models together.
///
/// The generator caps the same total at [`u8::MAX`] because model ids are bytes.
pub const MAX_NAME_MODELS: usize = 255;

/// The smallest name-block shift the format accepts (2 names per block).
pub const MIN_NAME_BLOCK_SHIFT: u8 = 1;

/// The largest name-block shift the format accepts (256 names per block).
pub const MAX_NAME_BLOCK_SHIFT: u8 = 8;

const _: () = assert!(NAME_PAGE_COUNT == (UNICODE_LIMIT as usize >> NAME_PAGE_SHIFT) + 1);

/// The first Hangul syllable, `HANGUL SYLLABLE GA`.
pub const HANGUL_START: u32 = 0xAC00;
/// The last Hangul syllable.
pub const HANGUL_END: u32 = 0xD7A3;
/// The number of Hangul syllables in Unicode 18.0.0.
pub const HANGUL_COUNT: usize = 11_172;

/// Choseong (leading) jamo short names, in `Jamo_Short_Name` order.
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

/// Writes the Hangul name of `cp` into `out`, returning `false` outside the syllable range.
///
/// The trainer verifies its `DerivedName.txt` Hangul rows with this same function, so the
/// algorithmic spelling is guaranteed to match what the runtime answers.
///
/// # Panics
///
/// Panics only if the fixed jamo tables are changed to be shorter than Unicode's 19/21/28
/// entries; the index arithmetic is bounded by the syllable count.
#[must_use = "the return value reports whether `cp` has a Hangul name"]
pub fn write_hangul(cp: u32, out: &mut String) -> bool {
    if !(HANGUL_START..=HANGUL_END).contains(&cp) {
        return false;
    }
    let index = cp - HANGUL_START;
    out.push_str("HANGUL SYLLABLE ");
    out.push_str(JAMO_LEADING[usize::try_from(index / 588).expect("leading index")]);
    out.push_str(JAMO_VOWEL[usize::try_from((index % 588) / 28).expect("vowel index")]);
    out.push_str(JAMO_TRAILING[usize::try_from(index % 28).expect("trailing index")]);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_constants_are_self_consistent() {
        assert_eq!(GC_PAGES, 545);
        assert_eq!(NAME_PAGE_COUNT, GC_PAGES);
        assert_eq!(GC_PAGE_SHIFT, 11);
        assert_eq!(GC_SHORT_NAMES.len(), 30);
        assert!(GC_SHORT_NAMES.len() < 1usize << GC_ID_BITS);
        assert_eq!(1u16 << GC_ID_BITS, GC_ID_MASK + 1);
        assert_eq!(UNICODE_START, 0);
        assert_eq!(UNICODE_LIMIT, UNICODE_END + 1);
        assert_eq!(UNICODE_LIMIT as usize >> GC_SHIFT, GC_PAGES - 1);
        assert_eq!(
            HANGUL_END - HANGUL_START + 1,
            u32::try_from(HANGUL_COUNT).expect("the count fits in u32")
        );
        assert_eq!(
            DIRECTORY_AT + SECTION_COUNT * DIRECTORY_ENTRY_LEN,
            HEADER_LEN - 18,
            "the header is padded to one cache line after the directory"
        );
        assert_eq!(VERSION_AT + 6, SECTION_COUNT_AT);
    }

    #[test]
    fn layout_ids_round_trip_and_reject_unknown_ids() {
        for layout in [NameLayout::BlockLocal, NameLayout::Global] {
            assert_eq!(NameLayout::from_id(layout.id()), Some(layout));
        }
        assert_eq!(NameLayout::from_id(2), None);
        assert_eq!(NameLayout::from_id(u8::MAX), None);
        assert!(NameLayout::Global.is_global());
        assert!(!NameLayout::BlockLocal.is_global());
    }

    #[test]
    fn hangul_names_cover_the_range() {
        let mut out = String::new();
        assert!(write_hangul(0xAC00, &mut out));
        assert_eq!(out, "HANGUL SYLLABLE GA");
        out.clear();
        assert!(write_hangul(0xAC01, &mut out));
        assert_eq!(out, "HANGUL SYLLABLE GAG");
        out.clear();
        assert!(write_hangul(0xD7A3, &mut out));
        assert_eq!(out, "HANGUL SYLLABLE HIH");
        out.clear();
        assert!(!write_hangul(0xABFF, &mut out));
        assert_eq!(out, "");
    }

    /// Packs a header whose four sections are empty and start at `HEADER_LEN`.
    fn empty_image() -> Vec<u8> {
        let mut image = vec![0u8; HEADER_LEN];
        image[..MAGIC.len()].copy_from_slice(MAGIC);
        image[VERSION_AT..VERSION_AT + 2].copy_from_slice(&18u16.to_le_bytes());
        image[SECTION_COUNT_AT] = u8::try_from(SECTION_COUNT).unwrap();
        image[NAME_BLOCK_SHIFT_AT] = 6;
        image[GC_PAGE_SHIFT_AT] = GC_PAGE_SHIFT;
        image[NAME_LAYOUT_AT] = NameLayout::Global.id();
        for id in 0..SECTION_COUNT {
            let at = DIRECTORY_AT + id * DIRECTORY_ENTRY_LEN;
            image[at..at + 4].copy_from_slice(&u32::try_from(HEADER_LEN).unwrap().to_le_bytes());
            image[at + 4..at + 8].copy_from_slice(&0u32.to_le_bytes());
        }
        image
    }

    #[test]
    fn the_image_header_parses_every_field_and_rejects_bad_ones() {
        let image = empty_image();
        let header = Header::parse(&image).expect("a valid header");
        assert_eq!(header.version, (18, 0, 0));
        assert_eq!(header.block_shift, 6);
        assert_eq!(header.gc_page_shift, GC_PAGE_SHIFT);
        assert_eq!(header.layout, NameLayout::Global);
        assert_eq!(header.section(&image, NAMES), Some(&[][..]));
        assert_eq!(header.section(&image, SECTION_COUNT), None);

        let mut wrong_magic = image.clone();
        wrong_magic[0] = b'X';
        assert!(Header::parse(&wrong_magic).is_none());

        let mut wrong_layout = image.clone();
        wrong_layout[NAME_LAYOUT_AT] = 2;
        assert!(Header::parse(&wrong_layout).is_none());

        let mut wrong_shift = image.clone();
        wrong_shift[NAME_BLOCK_SHIFT_AT] = 0;
        assert!(Header::parse(&wrong_shift).is_none());

        let mut out_of_bounds = image;
        out_of_bounds[DIRECTORY_AT + 4..DIRECTORY_AT + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(Header::parse(&out_of_bounds).is_none());
        assert!(Header::parse(&[0u8; HEADER_LEN - 1]).is_none());
    }
}
