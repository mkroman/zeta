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

use bytes::Buf;
use zeta_unicode_format::huffman;
use zeta_unicode_format::{
    MAX_CODE_BITS, MAX_NAME_BLOCK_SHIFT, MAX_NAME_BYTES, MAX_NAME_MODELS, MIN_NAME_BLOCK_SHIFT,
    NAME_CONTEXT_MAP_LEN, NAME_INDEX_HEADER_LEN, NAME_MODEL_COUNT_LEN,
    NAME_MODEL_DIRECTORY_ENTRY_LEN, NAME_PAGE_COUNT, NAME_PAGE_DIRECTORY_ENTRY_LEN,
    NAME_PAGE_SHIFT, NAME_RUN_LEN, UNICODE_END,
};

pub use zeta_unicode_format::{NameLayout, write_hangul};

/// The width of the per-model fast decode table.
const TABLE_BITS: u32 = 6;
/// The number of entries in one fast decode table.
const TABLE_SIZE: usize = 1 << TABLE_BITS;

/// One canonical Huffman model.
#[derive(Clone, Copy)]
struct Model<'a> {
    /// The longest code in the model.
    max_bits: u8,
    /// `counts[length]` is the number of symbols with that length.
    counts: [u16; MAX_CODE_BITS as usize + 1],
    /// Symbols in canonical order.
    symbols: &'a [u8],
}

/// Fills the fast decode table for one model.
///
/// Every canonical code of at most [`TABLE_BITS`] bits owns the `2^(TABLE_BITS - length)`
/// table slots whose top bits equal it; a longer code leaves zero ("escape") slots.
fn build_table(model: &Model<'_>, table: &mut [u16]) {
    let max_bits = usize::from(model.max_bits).min(TABLE_BITS as usize);
    let mut code = 0u32;
    let mut symbol_start = 0usize;
    for length in 1..=max_bits {
        let count = usize::from(model.counts[length]);
        let span = 1usize << (TABLE_BITS as usize - length);
        for offset in 0..count {
            let Some(&symbol) = model.symbols.get(symbol_start + offset) else {
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
    /// First run index of every 2,048-code-point page.
    page_dir: &'a [u8],
    /// `{ u32 first_cp, u16 length, u16 base_id }` records.
    runs: &'a [u8],
    /// Number of run records.
    run_count: usize,
    /// The layout the section uses.
    layout: NameLayout,
    /// The `u32` bit offset of every name block (the checkpoint entry for global blocks).
    block_bits: &'a [u8],
    /// Previous-byte to model-id map.
    context_model: [u8; 256],
    /// The canonical models, in id order.
    models: Vec<Model<'a>>,
    /// Fast decode tables, one [`TABLE_SIZE`]-entry table per model.
    ///
    /// An entry is `(code_length << 8) | symbol`, or zero when the code is longer than
    /// [`TABLE_BITS`] and the canonical fallback is needed.
    tables: Vec<u16>,
    /// The global MSB-first bitstream.
    stream: &'a [u8],
    /// Names in the section.
    name_count: usize,
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

/// The name section's fixed header and index.
struct Header<'a> {
    /// Names in the section.
    name_count: usize,
    /// Number of run records.
    run_count: usize,
    /// First run index of every 2,048-code-point page.
    page_dir: &'a [u8],
    /// `{ u32 first_cp, u16 length, u16 base_id }` records.
    runs: &'a [u8],
    /// The `u32` bit offset of every name block.
    block_bits: &'a [u8],
    /// Offset of the model area.
    models_at: usize,
}

impl<'a> Header<'a> {
    /// Parses the fixed header, page directory, runs and block offsets.
    fn parse(section: &'a [u8], block_shift: u8) -> Option<Self> {
        if !(MIN_NAME_BLOCK_SHIFT..=MAX_NAME_BLOCK_SHIFT).contains(&block_shift) {
            return None;
        }
        let mut reader = section;
        let name_count = usize::try_from(reader.try_get_u32_le().ok()?).ok()?;
        let block_count = usize::try_from(reader.try_get_u32_le().ok()?).ok()?;
        let run_count = usize::from(reader.try_get_u16_le().ok()?);
        let page_count = usize::from(reader.try_get_u16_le().ok()?);
        if name_count == 0 || page_count != NAME_PAGE_COUNT || run_count == 0 {
            return None;
        }
        let block_size = 1usize << block_shift;
        if block_count != name_count.div_ceil(block_size) {
            return None;
        }

        let page_dir_at = NAME_INDEX_HEADER_LEN;
        let runs_at =
            page_dir_at.checked_add(page_count.checked_mul(NAME_PAGE_DIRECTORY_ENTRY_LEN)?)?;
        let runs_end = runs_at.checked_add(run_count.checked_mul(NAME_RUN_LEN)?)?;
        let starts_at = runs_end.checked_add(3)? & !3;
        let starts_end = starts_at.checked_add(block_count.checked_mul(4)?)?;
        if starts_end > section.len() {
            return None;
        }

        let page_dir = section.get(page_dir_at..runs_at)?;
        let runs = section.get(runs_at..runs_end)?;
        validate_index(page_dir, runs, run_count, name_count)?;

        Some(Self {
            name_count,
            run_count,
            page_dir,
            runs,
            block_bits: section.get(starts_at..starts_end)?,
            models_at: starts_end,
        })
    }
}

/// Checks the run records and page directory the encoder writes.
///
/// A run must start after the previous one ends (so the runs are strictly ordered and
/// disjoint), end inside the Unicode range and continue the name-id sequence at exactly
/// `base_id`; together the runs must cover all `name_count` ids. The page directory must be a
/// non-decreasing list of run indices at most `run_count`. Every image the encoder produces
/// satisfies these, and a section that does not is rejected before `id_of` can index outside
/// the run array or the name table or resolve to another run's name.
fn validate_index(page_dir: &[u8], runs: &[u8], run_count: usize, name_count: usize) -> Option<()> {
    let mut previous_end = None;
    let mut expected_base = 0usize;
    for record in runs.as_chunks::<NAME_RUN_LEN>().0 {
        let mut record = record.as_slice();
        let first = record.try_get_u32_le().ok()?;
        let length = u32::from(record.try_get_u16_le().ok()?);
        let base = usize::from(record.try_get_u16_le().ok()?);
        if length == 0 {
            return None;
        }
        let end = first.checked_add(length - 1)?;
        let length = usize::try_from(length).ok()?;
        if previous_end.is_some_and(|previous| first <= previous)
            || end > UNICODE_END
            || base != expected_base
            || base.checked_add(length)? > name_count
        {
            return None;
        }
        previous_end = Some(end);
        expected_base = expected_base.checked_add(length)?;
    }
    if expected_base != name_count {
        return None;
    }

    let mut previous_run = 0usize;
    for entry in page_dir.as_chunks::<NAME_PAGE_DIRECTORY_ENTRY_LEN>().0 {
        let mut entry = entry.as_slice();
        let run = usize::from(entry.try_get_u16_le().ok()?);
        if run < previous_run || run > run_count {
            return None;
        }
        previous_run = run;
    }
    Some(())
}

/// The decoded context models and the bitstream they decode.
struct ModelSet<'a> {
    /// Previous-byte to model-id map.
    context: [u8; 256],
    /// The canonical models, in id order.
    models: Vec<Model<'a>>,
    /// Fast decode tables, one [`TABLE_SIZE`]-entry table per model.
    tables: Vec<u16>,
    /// The global MSB-first bitstream.
    stream: &'a [u8],
}

impl<'a> ModelSet<'a> {
    /// Parses the context map, model directory and model blobs, and builds the fast tables.
    fn parse(section: &'a [u8], models_at: usize) -> Option<Self> {
        let context_map = section.get(models_at..models_at.checked_add(NAME_CONTEXT_MAP_LEN)?)?;
        let count_at = models_at.checked_add(NAME_CONTEXT_MAP_LEN)?;
        let mut count_bytes = section.get(count_at..)?;
        let model_count = usize::from(count_bytes.try_get_u16_le().ok()?);
        if model_count == 0 || model_count > MAX_NAME_MODELS {
            return None;
        }
        let directory_at = models_at.checked_add(NAME_CONTEXT_MAP_LEN + NAME_MODEL_COUNT_LEN)?;
        let blobs_at =
            directory_at.checked_add(model_count.checked_mul(NAME_MODEL_DIRECTORY_ENTRY_LEN)?)?;
        if blobs_at > section.len() {
            return None;
        }

        let mut models = Vec::with_capacity(model_count);
        let mut expected = 0usize;
        for index in 0..model_count {
            let entry_at =
                directory_at.checked_add(index.checked_mul(NAME_MODEL_DIRECTORY_ENTRY_LEN)?)?;
            let mut entry = section.get(entry_at..)?;
            let relative = usize::from(entry.try_get_u16_le().ok()?);
            if relative != expected {
                return None;
            }
            let at = blobs_at.checked_add(relative)?;
            let max_bits = *section.get(at)?;
            if max_bits == 0 || max_bits > MAX_CODE_BITS {
                return None;
            }
            let model_counts_at = at.checked_add(1)?;
            let symbols_at = model_counts_at.checked_add(usize::from(max_bits) * 2)?;
            let mut counts = [0u16; MAX_CODE_BITS as usize + 1];
            let mut symbol_count = 0usize;
            for (length, slot) in counts
                .iter_mut()
                .enumerate()
                .take(usize::from(max_bits) + 1)
                .skip(1)
            {
                let length_at = model_counts_at.checked_add((length - 1).checked_mul(2)?)?;
                let mut length_bytes = section.get(length_at..)?;
                let count = length_bytes.try_get_u16_le().ok()?;
                *slot = count;
                symbol_count += usize::from(count);
            }
            if symbol_count > 256 {
                return None;
            }
            if symbol_count == 0 || !huffman::valid_codebook(&counts, max_bits) {
                return None;
            }
            let symbols_end = symbols_at.checked_add(symbol_count)?;
            let symbols = section.get(symbols_at..symbols_end)?;
            models.push(Model {
                max_bits,
                counts,
                symbols,
            });
            expected = symbols_end.checked_sub(blobs_at)?;
        }
        let stream_at = blobs_at.checked_add(expected)?;
        if stream_at > section.len() {
            return None;
        }
        let context: [u8; 256] = context_map.try_into().ok()?;
        if context
            .iter()
            .any(|&model| usize::from(model) >= model_count)
        {
            return None;
        }
        let mut tables = vec![0u16; model_count * TABLE_SIZE];
        for (index, model) in models.iter().enumerate() {
            build_table(
                model,
                &mut tables[index * TABLE_SIZE..(index + 1) * TABLE_SIZE],
            );
        }

        Some(Self {
            context,
            models,
            tables,
            stream: section.get(stream_at..)?,
        })
    }
}

impl<'a> Names<'a> {
    /// Parses and validates a name section.
    ///
    /// `block_shift` is the flat image header's `name_block_shift` (6 for the shipped 64-name
    /// blocks).
    #[must_use]
    pub(crate) fn new(section: &'a [u8], block_shift: u8, layout: NameLayout) -> Option<Self> {
        let header = Header::parse(section, block_shift)?;
        let models = ModelSet::parse(section, header.models_at)?;
        Some(Self {
            page_dir: header.page_dir,
            runs: header.runs,
            run_count: header.run_count,
            layout,
            block_bits: header.block_bits,
            context_model: models.context,
            models: models.models,
            tables: models.tables,
            stream: models.stream,
            name_count: header.name_count,
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
        if cp > UNICODE_END {
            return None;
        }
        let page = usize::try_from(cp >> NAME_PAGE_SHIFT).ok()?;
        let page_at = page.checked_mul(2)?;
        let mut page_bytes = self.page_dir.get(page_at..)?;
        let mut run = usize::from(page_bytes.try_get_u16_le().ok()?);
        loop {
            if run >= self.run_count {
                return None;
            }
            let at = run.checked_mul(8)?;
            let mut record = self.runs.get(at..)?;
            let first = record.try_get_u32_le().ok()?;
            if cp < first {
                return None;
            }
            let length = u32::from(record.try_get_u16_le().ok()?);
            if cp - first < length {
                let base = u32::from(record.try_get_u16_le().ok()?);
                return base.checked_add(cp - first);
            }
            run += 1;
        }
    }

    /// Appends name `id` to `out`, returning `false` when the id or the section is invalid.
    #[must_use = "the return value reports whether the id was decoded"]
    pub(crate) fn write_id(&mut self, id: u32, out: &mut String) -> bool {
        let id = usize::try_from(id).expect("a u32 fits in usize");
        if id >= self.name_count {
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
            let mut bit_bytes = self.block_bits.get(bit_at..)?;
            let bit = usize::try_from(bit_bytes.try_get_u32_le().ok()?).ok()?;
            (block << self.shift, bit)
        };
        let mut length = if resume { self.cache_len } else { 0 };
        let mut bits = Bits::new(self.stream, bit);
        if global && !resume && block > 0 {
            length = self.decode_checkpoint(&mut bits)?;
        }
        for _ in first..=id {
            length = self.decode_entry(&mut bits, length, global)?;
        }
        self.cache_id = id;
        self.cache_bit = bits.pos;
        self.cache_len = length;
        Some(())
    }

    /// Decodes the in-stream checkpoint name of a global block.
    fn decode_checkpoint(&mut self, bits: &mut Bits<'_>) -> Option<usize> {
        let model = self.models.len().checked_sub(1)?;
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
    fn decode_entry(&mut self, bits: &mut Bits<'_>, length: usize, global: bool) -> Option<usize> {
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
            let model = usize::from(self.context_model[usize::from(context)]);
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
    fn symbol(&self, index: usize, bits: &mut Bits<'_>) -> Option<u8> {
        decode_symbol(&self.models, &self.tables, index, bits)
    }
}

/// Decodes one canonical symbol from model `index` using the fast table and the canonical
/// fallback for codes longer than [`TABLE_BITS`].
fn decode_symbol(
    models: &[Model<'_>],
    tables: &[u16],
    index: usize,
    bits: &mut Bits<'_>,
) -> Option<u8> {
    // A valid stream never needs more than its own bytes; the allowance only lets a corrupt
    // one fail cleanly instead of decoding zero padding forever.
    if bits.pos > bits.limit + 64 {
        return None;
    }
    let window = bits.peek();
    let byte = u8::try_from(window >> (32 - TABLE_BITS)).ok()?;
    let entry = *tables.get(index * TABLE_SIZE + usize::from(byte))?;
    if entry != 0 {
        bits.pos += usize::from(entry >> 8);
        return u8::try_from(entry & 0xFF).ok();
    }
    let model = models.get(index)?;
    let max_bits = usize::from(model.max_bits);
    let counts = model.counts.get(1..=max_bits)?;
    let mut first = 0u32;
    let mut offset = 0usize;
    for (index, &count) in counts.iter().enumerate() {
        let length = u32::try_from(index + 1).expect("a code length fits in u32");
        let code = window >> (32 - length);
        let count = u32::from(count);
        if code.wrapping_sub(first) < count {
            let index = offset + usize::try_from(code - first).ok()?;
            bits.pos += usize::try_from(length).expect("a code length fits in usize");
            return model.symbols.get(index).copied();
        }
        offset += usize::try_from(count).ok()?;
        first = (first + count) << 1;
    }
    None
}

/// An MSB-first bit reader that peeks 32 bits at a time.
///
/// A symbol decode does one eight-byte load and advances by the code length, instead of
/// touching a bit buffer once per bit. Reads past the end are zero-filled; the symbol
/// decoder rejects a decode that strays too far past the stream.
struct Bits<'a> {
    /// The bitstream.
    data: &'a [u8],
    /// The absolute bit position.
    pos: usize,
    /// The first bit position past the stream's last byte.
    limit: usize,
}

impl<'a> Bits<'a> {
    /// A reader starting at absolute bit `bit`.
    const fn new(data: &'a [u8], bit: usize) -> Self {
        Self {
            data,
            pos: bit,
            limit: data.len() * 8,
        }
    }

    /// Returns the next 32 bits, most significant first, zero-filled past the end.
    #[inline]
    fn peek(&self) -> u32 {
        let byte = self.pos >> 3;
        let skip = u32::try_from(self.pos & 7).expect("a bit offset fits");
        let word = word_at(self.data, byte);
        u32::try_from((word << skip) >> 32).expect("the top half of a u64 fits in u32")
    }
}

/// Reads eight big-endian bytes at `at`, zero-filling past the end.
fn word_at(data: &[u8], at: usize) -> u64 {
    if let Some(bytes) = data.get(at..at + 8) {
        return u64::from_be_bytes(bytes.try_into().expect("eight bytes"));
    }
    let mut word = 0u64;
    for index in 0..8 {
        word = (word << 8) | u64::from(data.get(at + index).copied().unwrap_or(0));
    }
    word
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits_peek_is_msb_first_and_zero_padded() {
        let data = [0b1010_0110, 0b1100_0011];
        let mut bits = Bits::new(&data, 0);
        assert_eq!(bits.peek(), 0xA6C3_0000);
        bits.pos = 8;
        assert_eq!(bits.peek(), 0xC300_0000);
        bits.pos = 16;
        assert_eq!(bits.peek(), 0);
        assert_eq!(bits.limit, 16);
    }

    #[test]
    fn bits_can_start_mid_byte() {
        let data = [0b1010_0110, 0b1100_0011];
        let bits = Bits::new(&data, 3);
        assert_eq!(bits.peek() >> 28, 0b0011);
    }

    #[test]
    fn bits_read_across_the_eight_byte_window() {
        let data: Vec<u8> = (0..32).collect();
        let mut bits = Bits::new(&data, 8);
        assert_eq!(bits.peek(), u32::from_be_bytes([1, 2, 3, 4]));
        bits.pos = 24;
        assert_eq!(bits.peek(), u32::from_be_bytes([3, 4, 5, 6]));
        bits.pos = data.len() * 8 - 4;
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

    /// Builds a name-section index. `Header::parse` does not read the model area.
    fn index_section(
        name_count: usize,
        block_shift: u8,
        runs: &[(u32, u16, u16)],
        page_dir: &[u16],
    ) -> Vec<u8> {
        let block_count = name_count.div_ceil(1usize << block_shift);
        let mut section = Vec::new();
        section.extend_from_slice(&u32::try_from(name_count).unwrap().to_le_bytes());
        section.extend_from_slice(&u32::try_from(block_count).unwrap().to_le_bytes());
        section.extend_from_slice(&u16::try_from(runs.len()).unwrap().to_le_bytes());
        section.extend_from_slice(&u16::try_from(NAME_PAGE_COUNT).unwrap().to_le_bytes());
        for &entry in page_dir {
            section.extend_from_slice(&entry.to_le_bytes());
        }
        for &(first, length, base) in runs {
            section.extend_from_slice(&first.to_le_bytes());
            section.extend_from_slice(&length.to_le_bytes());
            section.extend_from_slice(&base.to_le_bytes());
        }
        while !section.len().is_multiple_of(4) {
            section.push(0);
        }
        section.resize(section.len() + block_count * 4, 0);
        section
    }

    /// The page directory the encoder writes for `runs`.
    fn page_directory(runs: &[(u32, u16, u16)]) -> Vec<u16> {
        (0..NAME_PAGE_COUNT)
            .map(|page| {
                let base = u32::try_from(page).unwrap() << NAME_PAGE_SHIFT;
                let first = runs
                    .iter()
                    .position(|&(start, length, _)| start + u32::from(length) > base)
                    .unwrap_or(runs.len());
                u16::try_from(first).unwrap()
            })
            .collect()
    }

    #[test]
    fn name_index_records_are_validated() {
        let runs = [(0x41u32, 2u16, 0u16), (0x100, 1, 2)];
        let page_dir = page_directory(&runs);
        let valid = index_section(3, 1, &runs, &page_dir);
        assert!(Header::parse(&valid, 1).is_some());

        // Runs must start after their predecessor.
        let duplicate = [(0x41, 1, 0), (0x41, 1, 1)];
        assert!(Header::parse(&index_section(2, 1, &duplicate, &page_dir), 1).is_none());
        let out_of_order = [(0x100, 1, 2), (0x41, 2, 0)];
        assert!(Header::parse(&index_section(3, 1, &out_of_order, &page_dir), 1).is_none());

        // A run must cover at least one code point and end inside the Unicode range.
        let empty_run = [(0x41, 0, 0), (0x100, 1, 0)];
        assert!(Header::parse(&index_section(1, 1, &empty_run, &page_dir), 1).is_none());
        let past_end = [(0x10_FFFF, 2, 0)];
        assert!(
            Header::parse(
                &index_section(1, 1, &past_end, &page_directory(&past_end)),
                1
            )
            .is_none()
        );

        // A run must not map past the last name id.
        let past_names = [(0x41, 1, 3)];
        assert!(
            Header::parse(
                &index_section(3, 1, &past_names, &page_directory(&past_names)),
                1
            )
            .is_none()
        );

        // A corrupt base id must not point at another run's names.
        let wrong_base = [(0x41, 1, 1)];
        assert!(
            Header::parse(
                &index_section(2, 1, &wrong_base, &page_directory(&wrong_base)),
                1
            )
            .is_none()
        );
        // Overlapping runs would shadow the later run's names.
        let overlapping = [(0x41, 2, 0), (0x42, 1, 2)];
        assert!(
            Header::parse(
                &index_section(3, 1, &overlapping, &page_directory(&overlapping)),
                1
            )
            .is_none()
        );

        // The page directory must be non-decreasing and stay inside the run array.
        let mut decreasing = page_dir.clone();
        *decreasing.last_mut().unwrap() = 0;
        assert!(Header::parse(&index_section(3, 1, &runs, &decreasing), 1).is_none());
        let mut too_far = page_dir;
        too_far[0] = 3;
        assert!(Header::parse(&index_section(3, 1, &runs, &too_far), 1).is_none());
    }

    /// A `Names` with a preset cache entry and no stream, for the push path.
    fn cached_name(scratch: &[u8], cache_id: usize) -> Names<'static> {
        let mut buffer = [0u8; MAX_NAME_BYTES];
        buffer[..scratch.len()].copy_from_slice(scratch);
        Names {
            page_dir: &[],
            runs: &[],
            run_count: 0,
            layout: NameLayout::BlockLocal,
            block_bits: &[],
            context_model: [0; 256],
            models: Vec::new(),
            tables: Vec::new(),
            stream: &[],
            name_count: 1,
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
        let model = Model {
            max_bits: 3,
            counts,
            symbols: &symbols,
        };
        let mut table = vec![0u16; TABLE_SIZE];
        build_table(&model, &mut table);

        let mut writer = huffman::BitWriter::new();
        for (bits, length) in [(0b0, 1), (0b10, 2), (0b110, 3), (0b111, 3)] {
            writer.push(bits, length);
        }
        let stream = writer.finish();
        let mut bits = Bits::new(&stream, 0);
        for want in symbols {
            assert_eq!(
                decode_symbol(&[model], &table, 0, &mut bits),
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
        let model = Model {
            max_bits: 7,
            counts,
            symbols: &symbols,
        };
        let mut table = vec![0u16; TABLE_SIZE];
        build_table(&model, &mut table);

        let mut writer = huffman::BitWriter::new();
        writer.push(0b0, 1);
        writer.push(0b100_0000, 7);
        let stream = writer.finish();
        let mut bits = Bits::new(&stream, 0);
        assert_eq!(decode_symbol(&[model], &table, 0, &mut bits), Some(0));
        assert_eq!(decode_symbol(&[model], &table, 0, &mut bits), Some(1));
    }
}
