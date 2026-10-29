//! Decoding the Nomina name section without materializing the dictionary.
//!
//! The format is documented in `zeta_unicode_build::nomina::names`; this module is the
//! runtime half. A lookup resolves `cp -> run -> name id` with one page-directory read and a
//! short scan of the run records, then decodes at most one block. A cursor resumes
//! inside the block when consecutive ids are requested, so walking `.uni` from `U+0000`
//! decodes every entry exactly once.
//!
//! Both layouts are supported. The block-local layout resets the predecessor at each block
//! and seeds it with an empty name. The global layout front-codes every name against its
//! code-point predecessor and starts each block (after the first) with an in-stream checkpoint
//! name, decoded with a dedicated order-0 model. The flat image header's name-layout byte
//! selects the decoder path.
//!
//! `Names::new` validates the run records and the page directory before a lookup can use them.
//! No heap allocation happens in `Names::write_id` or `Names::id_of`: the only scratch is the
//! two fixed 256-byte name buffers inside `Names` (the current name and a saved shared suffix).

use std::str;

use zeta_unicode_format::{BitReader, MAX_NAME_BYTES, Model, NameIndex, NameModels};

#[cfg(test)]
use zeta_unicode_format::{MAX_CODE_BITS, huffman};

pub use zeta_unicode_format::{NameLayout, write_hangul};

/// The width of the per-model fast decode table.
const TABLE_BITS: u32 = 6;
/// The number of entries in one fast decode table.
const TABLE_SIZE: usize = 1 << TABLE_BITS;

/// Fills the fast decode table for one model.
///
/// Every canonical code of at most [`TABLE_BITS`] bits owns the `2^(TABLE_BITS - length)`
/// table slots whose top bits equal it; a longer code leaves zero ("escape") slots.
fn build_table(model: &Model<'_>, table: &mut [u16]) {
    let max_bits = usize::from(model.max_bits()).min(TABLE_BITS as usize);
    let counts = model.counts();
    let symbols = model.symbols();
    let mut code = 0u32;
    let mut symbol_start = 0usize;
    for (length, &count) in counts.iter().enumerate().take(max_bits + 1).skip(1) {
        let count = usize::from(count);
        let span = 1usize << (TABLE_BITS as usize - length);
        for offset in 0..count {
            let Some(&symbol) = symbols.get(symbol_start + offset) else {
                return;
            };
            let prefix = usize::try_from(code + u32::try_from(offset).expect("offset fits"))
                .expect("code fits")
                << (TABLE_BITS as usize - length);
            let entry = (u16::try_from(length).expect("length fits") << 8) | u16::from(symbol);
            for slot in table.iter_mut().skip(prefix).take(span) {
                *slot = entry;
            }
        }
        symbol_start += count;
        code = (code + u32::try_from(count).expect("count fits")) << 1;
    }
}

/// The decoded name section's fixed-size lookup state.
pub struct Names<'a> {
    /// The validated run index and block offsets.
    index: NameIndex<'a>,
    /// The validated context models and bitstream.
    models: NameModels<'a>,
    /// The layout the section uses.
    layout: NameLayout,
    /// Fast decode tables, one [`TABLE_SIZE`]-entry table per model.
    ///
    /// An entry is `(code_length << 8) | symbol`, or zero when the code is longer than
    /// [`TABLE_BITS`] and the canonical fallback is needed.
    tables: Vec<u16>,
    /// `id >> shift` selects a block.
    shift: u32,
    /// The current decoded name.
    scratch: [u8; MAX_NAME_BYTES],
    /// Scratch for the shared suffix, copied before the middle overwrites it.
    suffix_scratch: [u8; MAX_NAME_BYTES],
    /// The id whose name is in `scratch` (`usize::MAX` when none).
    cache_id: usize,
    /// The bit position just after that entry.
    cache_bit: usize,
    /// The length of that entry's name.
    cache_len: usize,
}

impl<'a> Names<'a> {
    /// Parses and validates a name section.
    ///
    /// `block_shift` is the flat image header's `name_block_shift` (6 for the shipped 64-name
    /// blocks).
    #[must_use]
    pub(crate) fn new(section: &'a [u8], block_shift: u8, layout: NameLayout) -> Option<Self> {
        let index = NameIndex::parse(section, block_shift)?;
        let models = NameModels::parse(section, index.models_at)?;
        let mut tables = vec![0u16; models.models.len() * TABLE_SIZE];
        for (model_index, model) in models.models.iter().enumerate() {
            build_table(
                model,
                &mut tables[model_index * TABLE_SIZE..(model_index + 1) * TABLE_SIZE],
            );
        }
        Some(Self {
            index,
            models,
            layout,
            tables,
            shift: u32::from(block_shift),
            scratch: [0; MAX_NAME_BYTES],
            suffix_scratch: [0; MAX_NAME_BYTES],
            cache_id: usize::MAX,
            cache_bit: 0,
            cache_len: 0,
        })
    }

    /// Returns the name id of `cp`, or `None` when it has no stored name.
    #[inline]
    pub(crate) fn id_of(&self, cp: u32) -> Option<u32> {
        self.index.id_of(cp)
    }

    /// Appends name `id` to `out`, returning `false` when the id or the section is invalid.
    #[must_use = "the return value reports whether the id was decoded"]
    pub(crate) fn write_id(&mut self, id: u32, out: &mut String) -> bool {
        let id = usize::try_from(id).expect("a u32 fits in usize");
        if id >= self.index.name_count {
            return false;
        }
        if self.cache_id == id {
            return self.push_cached(out);
        }
        if self.decode_id(id).is_none() {
            self.invalidate();
            return false;
        }
        self.push_cached(out)
    }

    /// Decodes name `id` into `scratch` and updates the sequential cursor.
    fn decode_id(&mut self, id: usize) -> Option<()> {
        let global = self.layout.is_global();
        let block = id >> self.shift;
        let resume = self.cache_id != usize::MAX
            && id > self.cache_id
            && block == (self.cache_id >> self.shift);
        let (first, bit) = if resume {
            (self.cache_id + 1, self.cache_bit)
        } else {
            let bit_at = block.checked_mul(4)?;
            let bit_bytes = self.index.block_bits.get(bit_at..)?;
            let bit =
                usize::try_from(u32::from_le_bytes(bit_bytes.get(..4)?.try_into().ok()?)).ok()?;
            (block << self.shift, bit)
        };
        let mut length = if resume { self.cache_len } else { 0 };
        let mut bits = BitReader::at(self.models.stream, bit);
        if global && !resume && block > 0 {
            length = self.decode_checkpoint(&mut bits)?;
        }
        for _ in first..=id {
            length = self.decode_entry(&mut bits, length, global)?;
        }
        self.cache_id = id;
        self.cache_bit = bits.position();
        self.cache_len = length;
        Some(())
    }

    /// Decodes the in-stream checkpoint name of a global block.
    fn decode_checkpoint(&mut self, bits: &mut BitReader<'_>) -> Option<usize> {
        let model = self.models.models.len().checked_sub(1)?;
        let mut length = 0usize;
        loop {
            let byte = self.symbol(model, bits)?;
            if byte == 0 {
                break;
            }
            *self.scratch.get_mut(length)? = byte;
            length += 1;
        }
        Some(length)
    }

    /// Decodes one front-coded entry, returning the new name length.
    fn decode_entry(
        &mut self,
        bits: &mut BitReader<'_>,
        length: usize,
        global: bool,
    ) -> Option<usize> {
        let prefix = usize::from(self.symbol(0, bits)?);
        let suffix = if global {
            usize::from(self.symbol(1, bits)?)
        } else {
            0
        };
        if prefix > length || suffix > length - prefix {
            return None;
        }
        if suffix > 0 {
            self.suffix_scratch[..suffix].copy_from_slice(&self.scratch[length - suffix..length]);
        }
        let mut new_length = prefix;
        let mut context = if prefix == 0 {
            0
        } else {
            *self.scratch.get(prefix - 1)?
        };
        loop {
            let model = usize::from(self.models.context[usize::from(context)]);
            let byte = self.symbol(model, bits)?;
            if byte == 0 {
                break;
            }
            *self.scratch.get_mut(new_length)? = byte;
            new_length += 1;
            context = byte;
        }
        if suffix > 0 {
            let end = new_length.checked_add(suffix)?;
            self.scratch
                .get_mut(new_length..end)?
                .copy_from_slice(&self.suffix_scratch[..suffix]);
            new_length = end;
        }
        Some(new_length)
    }

    /// Pushes the cached name into `out`, dropping the cursor when it is not valid UTF-8.
    ///
    /// A failed push must invalidate: `decode_id` has already recorded this id as the
    /// sequential predecessor, so a later lookup in the same block would otherwise resume from
    /// bytes that cannot be part of the output.
    fn push_cached(&mut self, out: &mut String) -> bool {
        if let Ok(text) = str::from_utf8(&self.scratch[..self.cache_len]) {
            out.push_str(text);
            return true;
        }
        self.invalidate();
        false
    }

    /// Drops the sequential cursor after a failed decode.
    const fn invalidate(&mut self) {
        self.cache_id = usize::MAX;
        self.cache_len = 0;
    }

    /// Decodes one canonical symbol from model `index`.
    #[inline]
    fn symbol(&self, index: usize, bits: &mut BitReader<'_>) -> Option<u8> {
        decode_symbol(&self.models.models, &self.tables, index, bits)
    }
}

/// Decodes one canonical symbol from model `index` using the fast table and the canonical
/// fallback for codes longer than [`TABLE_BITS`].
fn decode_symbol(
    models: &[Model<'_>],
    tables: &[u16],
    index: usize,
    bits: &mut BitReader<'_>,
) -> Option<u8> {
    // A valid stream never needs more than its own bytes; the allowance only lets a corrupt
    // one fail cleanly instead of decoding zero padding forever.
    if bits.position() > bits.limit() + 64 {
        return None;
    }
    let window = bits.peek();
    let byte = u8::try_from(window >> (32 - TABLE_BITS)).ok()?;
    let entry = *tables.get(index * TABLE_SIZE + usize::from(byte))?;
    if entry != 0 {
        bits.advance(usize::from(entry >> 8));
        return u8::try_from(entry & 0xFF).ok();
    }
    let model = models.get(index)?;
    let (symbol, length) = model.decode_prefix(window)?;
    bits.advance(usize::from(length));
    Some(symbol)
}

#[cfg(test)]
mod tests {
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

    /// A `Names` with a preset cache entry and no stream, for the push path.
    fn cached_name(scratch: &[u8], cache_id: usize) -> Names<'static> {
        let mut buffer = [0u8; MAX_NAME_BYTES];
        buffer[..scratch.len()].copy_from_slice(scratch);
        Names {
            index: NameIndex {
                name_count: 1,
                block_count: 1,
                run_count: 0,
                page_dir: &[],
                runs: &[],
                block_bits: &[],
                models_at: 0,
            },
            models: NameModels {
                context: [0; 256],
                models: Vec::new(),
                stream: &[],
            },
            layout: NameLayout::BlockLocal,
            tables: Vec::new(),
            shift: 1,
            scratch: buffer,
            suffix_scratch: [0; MAX_NAME_BYTES],
            cache_id,
            cache_bit: 0,
            cache_len: scratch.len(),
        }
    }

    #[test]
    fn an_invalid_utf8_cached_name_drops_the_cursor() {
        // A cache hit goes straight to `push_cached`; a non-UTF-8 entry must not leave the
        // sequential cursor pointing at bytes a later lookup would resume from.
        let mut names = cached_name(&[0xFF], 0);
        let mut out = String::from("unchanged");
        assert!(!names.write_id(0, &mut out));
        assert_eq!(out, "unchanged");
        assert_eq!(names.cache_id, usize::MAX);
        assert_eq!(names.cache_len, 0);

        // A valid cached entry still answers and keeps its cursor.
        let mut names = cached_name(b"LATIN CAPITAL LETTER A", 0);
        let mut out = String::new();
        assert!(names.write_id(0, &mut out));
        assert_eq!(out, "LATIN CAPITAL LETTER A");
        assert_eq!(names.cache_id, 0);
    }

    #[test]
    fn fast_table_decoding_matches_the_canonical_walk() {
        let mut counts = [0u16; MAX_CODE_BITS as usize + 1];
        counts[1] = 1;
        counts[2] = 1;
        counts[3] = 2;
        let symbols = [0u8, 1, 2, 3];
        let model = Model::new(3, &counts, &symbols).expect("a complete codebook");
        let mut table = vec![0u16; TABLE_SIZE];
        build_table(&model, &mut table);

        let mut writer = huffman::BitWriter::new();
        for (bits, length) in [(0b0, 1), (0b10, 2), (0b110, 3), (0b111, 3)] {
            writer.push(bits, length);
        }
        let stream = writer.finish();
        let mut bits = BitReader::new(&stream);
        for want in symbols {
            assert_eq!(
                decode_symbol(std::slice::from_ref(&model), &table, 0, &mut bits),
                Some(want),
                "symbol {want}"
            );
        }
    }

    #[test]
    fn a_code_longer_than_the_fast_table_falls_back_to_the_canonical_walk() {
        let mut counts = [0u16; MAX_CODE_BITS as usize + 1];
        counts[1] = 1;
        counts[7] = 1;
        let symbols = [0u8, 1];
        let model = Model::new(7, &counts, &symbols).expect("a sparse codebook");
        let mut table = vec![0u16; TABLE_SIZE];
        build_table(&model, &mut table);

        let mut writer = huffman::BitWriter::new();
        writer.push(0b0, 1);
        writer.push(0b100_0000, 7);
        let stream = writer.finish();
        let mut bits = BitReader::new(&stream);
        assert_eq!(
            decode_symbol(std::slice::from_ref(&model), &table, 0, &mut bits),
            Some(0)
        );
        assert_eq!(
            decode_symbol(std::slice::from_ref(&model), &table, 0, &mut bits),
            Some(1)
        );
    }
}
