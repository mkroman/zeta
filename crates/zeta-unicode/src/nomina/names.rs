//! Decoding the Nomina name section without materializing the dictionary.
//!
//! The format is documented in `zeta_unicode_build::nomina::names`; the front-coding core is
//! [`zeta_unicode_format::NameDecoder`] and this module is the runtime wrapper. A lookup
//! resolves `cp -> run -> name id` with one page-directory read and a short scan of the run
//! records, then decodes at most one block. A cursor resumes inside the block when consecutive
//! ids are requested, so walking `.uni` from `U+0000` decodes every entry exactly once.
//!
//! [`Names::new`] validates the run records and the page directory before a lookup can use them.
//! No heap allocation happens in [`Names::write_id`] or [`Names::id_of`]: the only scratch is
//! the shared decoder's two fixed 256-byte name buffers (the current name and a saved shared
//! suffix). The decoder itself is per-thread, so name lookups need no lock.

pub(crate) use zeta_unicode_format::{NameLayout, write_hangul};

use zeta_unicode_format::NameDecoder;

/// A per-thread Nomina name decoder.
///
/// The shared decoder already exposes the fixed-size scratch buffers, sequential cursor and
/// fast decode tables; the wrapper only narrows the visibility to this crate.
pub(crate) struct Names<'a> {
    /// The shared front-coding decoder.
    decoder: NameDecoder<'a>,
}

impl<'a> Names<'a> {
    /// Parses and validates a name section.
    ///
    /// `block_shift` is the flat image header's `name_block_shift` (6 for the shipped 64-name
    /// blocks). Returns `None` for a truncated or inconsistent index or model area.
    #[must_use]
    pub(crate) fn new(section: &'a [u8], block_shift: u8, layout: NameLayout) -> Option<Self> {
        Some(Self {
            decoder: NameDecoder::new(section, block_shift, layout)?,
        })
    }

    /// Returns the name id of `cp`, or `None` when it has no stored name.
    #[inline]
    #[must_use]
    pub(crate) fn id_of(&self, cp: u32) -> Option<u32> {
        self.decoder.id_of(cp)
    }

    /// Appends name `id` to `out`, returning `false` when the id or the section is invalid.
    #[must_use = "the return value reports whether the id was decoded"]
    pub(crate) fn write_id(&mut self, id: u32, out: &mut String) -> bool {
        self.decoder.write_id(id, out)
    }
}

#[cfg(test)]
mod tests {
    use zeta_unicode_format::BitReader;

    use super::*;

    #[test]
    fn bits_peek_is_msb_first_and_zero_padded() {
        let data = [0b1010_0110, 0b1100_0011];
        let bits = BitReader::new(&data);
        assert_eq!(bits.peek(), 0xA6C3_0000);
        let bits = BitReader::at(&data, 8);
        assert_eq!(bits.peek(), 0xC300_0000);
        let bits = BitReader::at(&data, 16);
        assert_eq!(bits.peek(), 0);
        assert_eq!(bits.limit(), 16);
    }

    #[test]
    fn bits_can_start_mid_byte() {
        let data = [0b1010_0110, 0b1100_0011];
        let bits = BitReader::at(&data, 3);
        assert_eq!(bits.peek() >> 28, 0b0011);
    }

    #[test]
    fn bits_read_across_the_eight_byte_window() {
        let data: Vec<u8> = (0..32).collect();
        let bits = BitReader::at(&data, 8);
        assert_eq!(bits.peek(), u32::from_be_bytes([1, 2, 3, 4]));
        let bits = BitReader::at(&data, 24);
        assert_eq!(bits.peek(), u32::from_be_bytes([3, 4, 5, 6]));
        let bits = BitReader::at(&data, data.len() * 8 - 4);
        assert_eq!(bits.peek(), 0xF000_0000);
    }

    #[test]
    fn hangul_names_use_the_shared_tables() {
        let mut out = String::new();
        assert!(write_hangul(0xAC00, &mut out));
        assert_eq!(out, "HANGUL SYLLABLE GA");
        out.clear();
        assert!(write_hangul(0xD7A3, &mut out));
        assert_eq!(out, "HANGUL SYLLABLE HIH");
    }
}
