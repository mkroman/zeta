//! Decoding the Genesis name section without materializing the dictionary.
//!
//! The format is documented in `zeta_unicode_build::genesis::names`; this module is the
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
//! No heap allocation happens in `Names::write_id` or `Names::id_of`: the only scratch is
//! the fixed 256-byte name buffer inside `Names`.

use std::str;

use zeta_unicode_format::{HANGUL_END, HANGUL_START, UNICODE_END};

/// The number of name pages, one per 2,048 code points.
pub const PAGE_COUNT: usize = 545;
/// The code-point page shift of the run directory.
pub const PAGE_SHIFT: u32 = 11;
/// The largest name the fixed scratch buffer holds, in bytes.
pub const MAX_NAME_BYTES: usize = 256;
/// The largest canonical code the decoder accepts, in bits.
const MAX_CODE_BITS: u8 = 16;
/// The most models a section may declare: the prefix model, 256 byte contexts and the
/// generic context.
const MAX_MODELS: usize = 258;
/// The width of the per-model fast decode table.
const TABLE_BITS: u32 = 6;
/// The number of entries in one fast decode table.
const TABLE_SIZE: usize = 1 << TABLE_BITS;

/// Choongbae (leading) jamo short names, in `Jamo_Short_Name` order.
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

/// One canonical Huffman model.
#[derive(Clone, Copy)]
struct Model {
    /// The longest code in the model.
    max_bits: u8,
    /// `counts[length]` is the number of symbols with that length.
    counts: [u16; MAX_CODE_BITS as usize + 1],
    /// Symbols in canonical order.
    symbols: &'static [u8],
}

/// Fills the fast decode table for one model.
///
/// Every canonical code of at most [`TABLE_BITS`] bits owns the `2^(TABLE_BITS - length)`
/// table slots whose top bits equal it; a longer code leaves zero ("escape") slots.
fn build_table(model: &Model, table: &mut [u16]) {
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

/// The name layout an image was built with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    /// The predecessor resets at every block boundary.
    BlockLocal,
    /// Every name uses its code-point predecessor; each block has a checkpoint.
    Global,
}

impl Layout {
    /// Parses the image header's name-layout byte.
    pub(crate) const fn from_id(id: u8) -> Option<Self> {
        match id {
            0 => Some(Self::BlockLocal),
            1 => Some(Self::Global),
            _ => None,
        }
    }

    /// Is this the global checkpointed layout?
    const fn is_global(self) -> bool {
        matches!(self, Self::Global)
    }
}

/// The decoded name section's fixed-size lookup state.
pub(crate) struct Names {
    /// First run index of every 2,048-code-point page.
    page_dir: &'static [u8],
    /// `{ u32 first_cp, u16 length, u16 base_id }` records.
    runs: &'static [u8],
    /// Number of run records.
    run_count: usize,
    /// The layout the section uses.
    layout: Layout,
    /// The `u32` bit offset of every name block (the checkpoint entry for global blocks).
    block_bits: &'static [u8],
    /// Previous-byte to model-id map.
    context_model: [u8; 256],
    /// The canonical models, in id order.
    models: Vec<Model>,
    /// Fast decode tables, one [`TABLE_SIZE`]-entry table per model.
    ///
    /// An entry is `(code_length << 8) | symbol`, or zero when the code is longer than
    /// [`TABLE_BITS`] and the canonical fallback is needed.
    tables: Vec<u16>,
    /// The global MSB-first bitstream.
    stream: &'static [u8],
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

impl Names {
    /// Parses and validates a name section.
    ///
    /// `block_shift` is the flat image header's `name_block_shift` (6 for the shipped 64-name
    /// blocks).
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "header, directory, runs, models and table validation are one parse"
    )]
    pub(crate) fn new(section: &'static [u8], block_shift: u8, layout: Layout) -> Option<Self> {
        if block_shift == 0 || block_shift > 8 {
            return None;
        }
        let name_count = usize::try_from(u32_at(section, 0)?).ok()?;
        let block_count = usize::try_from(u32_at(section, 4)?).ok()?;
        let run_count = usize::from(u16_at(section, 8)?);
        let page_count = usize::from(u16_at(section, 10)?);
        if name_count == 0 || page_count != PAGE_COUNT || run_count == 0 {
            return None;
        }
        let block_size = 1usize << block_shift;
        if block_count != name_count.div_ceil(block_size) {
            return None;
        }

        let page_dir_at = 12;
        let runs_at = page_dir_at + page_count * 2;
        let runs_end = runs_at + run_count * 8;
        let starts_at = (runs_end + 3) & !3;
        let starts_end = starts_at + block_count * 4;
        if starts_end > section.len() {
            return None;
        }

        let block_bits = section.get(starts_at..starts_end)?;
        let models_at = starts_end;

        let context_map = section.get(models_at..models_at + 256)?;
        let model_count = usize::from(u16_at(section, models_at + 256)?);
        if model_count == 0 || model_count > MAX_MODELS {
            return None;
        }
        let directory_at = models_at + 258;
        let blobs_at = directory_at.checked_add(model_count * 2)?;
        if blobs_at > section.len() {
            return None;
        }

        let mut models = Vec::with_capacity(model_count);
        let mut expected = 0usize;
        for index in 0..model_count {
            let relative = usize::from(u16_at(section, directory_at + index * 2)?);
            if relative != expected {
                return None;
            }
            let at = blobs_at.checked_add(relative)?;
            let max_bits = *section.get(at)?;
            if max_bits == 0 || max_bits > MAX_CODE_BITS {
                return None;
            }
            let counts_at = at + 1;
            let symbols_at = counts_at.checked_add(usize::from(max_bits) * 2)?;
            let mut counts = [0u16; MAX_CODE_BITS as usize + 1];
            let mut symbol_count = 0usize;
            for (length, slot) in counts
                .iter_mut()
                .enumerate()
                .take(usize::from(max_bits) + 1)
                .skip(1)
            {
                let count = u16_at(section, counts_at + (length - 1) * 2)?;
                *slot = count;
                symbol_count += usize::from(count);
            }
            if symbol_count > 256 {
                return None;
            }
            let symbols_end = symbols_at.checked_add(symbol_count)?;
            let symbols = section.get(symbols_at..symbols_end)?;
            models.push(Model {
                max_bits,
                counts,
                symbols,
            });
            expected = symbols_end - blobs_at;
        }
        let stream_at = blobs_at.checked_add(expected)?;
        if stream_at > section.len() {
            return None;
        }
        let context_model: [u8; 256] = context_map.try_into().ok()?;
        if context_model
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
            page_dir: section.get(page_dir_at..runs_at)?,
            runs: section.get(runs_at..runs_end)?,
            run_count,
            layout,
            block_bits,
            context_model,
            models,
            tables,
            stream: section.get(stream_at..)?,
            name_count,
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
        let page = usize::try_from(cp >> PAGE_SHIFT).ok()?;
        let mut run = usize::from(u16_at(self.page_dir, page * 2)?);
        loop {
            if run >= self.run_count {
                return None;
            }
            let at = run * 8;
            let first = u32_at(self.runs, at)?;
            if cp < first {
                return None;
            }
            let length = u32::from(u16_at(self.runs, at + 4)?);
            if cp - first < length {
                let base = u32::from(u16_at(self.runs, at + 6)?);
                return base.checked_add(cp - first);
            }
            run += 1;
        }
    }

    /// Appends name `id` to `out`, returning `false` when the id or the section is invalid.
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
            let bit = usize::try_from(u32_at(self.block_bits, block * 4)?).ok()?;
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

    /// Pushes the cached name into `out`.
    fn push_cached(&self, out: &mut String) -> bool {
        let Ok(text) = str::from_utf8(&self.scratch[..self.cache_len]) else {
            return false;
        };
        out.push_str(text);
        true
    }

    /// Drops the sequential cursor after a failed decode.
    const fn invalidate(&mut self) {
        self.cache_id = usize::MAX;
        self.cache_len = 0;
    }

    /// Decodes one canonical symbol from model `index`.
    #[inline]
    fn symbol(&self, index: usize, bits: &mut Bits<'_>) -> Option<u8> {
        // A valid stream never needs more than its own bytes; the allowance only lets a
        // corrupt one fail cleanly instead of decoding zero padding forever.
        if bits.pos > bits.limit + 64 {
            return None;
        }
        let window = bits.peek();
        let byte = u8::try_from(window >> (32 - TABLE_BITS)).ok()?;
        let entry = *self.tables.get(index * TABLE_SIZE + usize::from(byte))?;
        if entry != 0 {
            bits.pos += usize::from(entry >> 8);
            return u8::try_from(entry & 0xFF).ok();
        }
        let model = self.models.get(index)?;
        let max_bits = usize::from(model.max_bits);
        let counts = model.counts.get(1..=max_bits)?;
        let mut first = 0u32;
        let mut offset = 0usize;
        for (index, &count) in counts.iter().enumerate() {
            let length = index + 1;
            #[allow(
                clippy::cast_possible_truncation,
                reason = "the code length is at most 24"
            )]
            let code = window >> (32 - length as u32);
            let count = u32::from(count);
            if code.wrapping_sub(first) < count {
                let index = offset + usize::try_from(code - first).ok()?;
                bits.pos += length;
                return model.symbols.get(index).copied();
            }
            offset += usize::try_from(count).ok()?;
            first = (first + count) << 1;
        }
        None
    }
}

/// Writes the Hangul name of `cp` into `out`, returning `false` outside the syllable range.
pub(crate) fn write_hangul(cp: u32, out: &mut String) -> bool {
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

/// Reads a little-endian `u16`.
#[inline]
fn u16_at(buf: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        buf.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}

/// Reads a little-endian `u32`.
#[inline]
fn u32_at(buf: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        buf.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
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
}
