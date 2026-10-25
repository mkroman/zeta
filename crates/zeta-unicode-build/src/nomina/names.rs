//! The name section: a random-access, order-1 canonical-Huffman front-coded dictionary.
//!
//! # Section layout
//!
//! All integers are little-endian. `page_count` is 544 pages of 2,048 code points plus one
//! sentinel page (`0x11_0000 >> 11` rounded up), and every name id is the position of its name
//! in ascending code-point order.
//!
//! ```text
//! off  field
//!   0  u32 name_count
//!   4  u32 block_count                (ceil(name_count / 2^shift))
//!   8  u16 run_count
//!  10  u16 page_count                 (545)
//!  12  u16 page_directory[page_count]  first run whose end is strictly after page << 11
//!      run[run_count]                 { u32 first_cp, u16 length, u16 base_id }
//!      (padding to a u32 boundary)
//!      u32 block_bits[block_count]    MSB-first bit offset of each block
//!      model area (see below)
//!      the MSB-first Huffman bitstream
//! ```
//!
//! A run covers `first_cp .. first_cp + length` (exclusive) and maps those code points to
//! `base_id .. base_id + length`. A lookup reads one page-directory entry and scans forward
//! through at most a handful of runs, so it never binary-searches 719 records.
//!
//! # Model area
//!
//! ```text
//!   u8  context_model[256]   previous byte -> model id
//!   u16 model_count
//!   u16 model_offset[model_count]   from the start of the model blobs
//!   model blobs
//! ```
//!
//! One model is `u8 max_bits`, then `u16 counts[1..=max_bits]`, then the `sum(counts)`
//! symbols in canonical order (shortest code first, ties by symbol value). Model 0 is the
//! prefix-length model; models `1..=C` are the `C` byte contexts in ascending previous-byte
//! order; model `C + 1` is the generic context. Names are ASCII, so the start-of-name
//! context is the byte value 0, which cannot occur inside a stored name. Every other byte
//! that has never preceded a name byte maps to the generic model; the encoder never selects
//! it, but a corrupt stream still decodes into a valid (if meaningless) symbol.
//!
//! # Layouts
//!
//! [`NameLayout::BlockLocal`] resets the predecessor at every block boundary: the first name
//! of a block has prefix 0, so a block is independently decodable.
//!
//! [`NameLayout::Global`] front-codes every name against its immediate code-point
//! predecessor and shares both the prefix and the suffix (the incumbent generator's streams).
//! Each block then starts with an in-stream checkpoint: the predecessor name coded as a full
//! name under a dedicated order-0 model. A block is still independently decodable — decode the
//! checkpoint, then at most `2^block_shift` entries — and the global stream avoids 1,107
//! predecessor resets. At 32-name blocks the checkpoint costs about 16 KB and saves about
//! 37 KB of front-coded middle bytes.
//!
//! # Entries
//!
//! A block holds at most `2^block_shift` names (the default profile ships 64-name blocks,
//! `name_block_shift = 6`). A block-local entry
//! is one prefix-length symbol `p` (0..=255) followed by the bytes of the name after its first
//! `p` bytes, followed by one zero terminator. A global entry adds a shared-suffix length
//! symbol between the prefix and the middle, and reconstructs the suffix from the predecessor.
//! The context of the first middle byte is the byte at `p - 1` of the predecessor, or 0 when
//! `p == 0`; every middle byte then selects the next context. The first name of a block-local
//! block always has `p == 0`; a global block's checkpoint supplies the predecessor.
//!
//! The block offsets make a lookup decode at most one block. A sequential cursor
//! (in the runtime) additionally resumes inside a block, which is what makes `.uni`-style
//! increasing-id walks decode every entry once.
//!
//! The global layout is smaller at every measured shift (32/64/128/256); at 32 names it is
//! 184,467 B versus block-local's 205,441 B, with the same random-lookup latency.
//!
//! # Corpus
//!
//! The corpus is exactly the current generator's: every individually named code point from
//! `DerivedName.txt`, the `NameAliases.txt` aliases folded in for code points without a
//! name, and the Hangul syllables and `*` patterns excluded from the section (they stay
//! algorithmic). That is 35,416 names in Unicode 18.0.0. Names are front-coded in code-point
//! order; block boundaries are fixed every `2^block_shift` names.
//!
//! [`encode_with_block_shift`] and [`encode_global_with_block_shift`] are the test and
//! measurement entry points; training calls [`encode_global_corpus`] after parsing the corpus
//! once. The `*_with_block_shift` forms return the size breakdown the layout study reports.

use bytes::{Buf, BufMut};

use zeta_unicode_format::huffman::{BitWriter, Code, canonical_codes, code_lengths};
use zeta_unicode_format::{
    HANGUL_COUNT, HANGUL_END, HANGUL_START, MAX_CODE_BITS, MAX_NAME_BYTES, MAX_NAME_MODELS,
    NAME_CONTEXT_MAP_LEN, NAME_INDEX_HEADER_LEN, NAME_MODEL_COUNT_LEN,
    NAME_MODEL_DIRECTORY_ENTRY_LEN, NAME_PAGE_COUNT, NAME_PAGE_DIRECTORY_ENTRY_LEN,
    NAME_PAGE_SHIFT, NAME_RUN_LEN, UNICODE_END, write_hangul, write_pattern_name,
};

use crate::ucd::Inputs;

pub use zeta_unicode_format::{
    MAX_NAME_BLOCK_SHIFT as MAX_BLOCK_SHIFT, MIN_NAME_BLOCK_SHIFT as MIN_BLOCK_SHIFT, NameLayout,
};

/// The default block shift of the unparameterized helpers (32 names).
#[cfg(test)]
pub const NAME_BLOCK_SHIFT: u8 = 5;
/// The number of `u16` length counts in one model, including the unused zero entry.
const CODE_LENGTH_SLOTS: usize = MAX_CODE_BITS as usize + 1;

/// The statistic and size breakdown of one encoded name section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NameStats {
    /// Names in the section.
    pub name_count: usize,
    /// Front-coded blocks.
    pub block_count: usize,
    /// Maximal runs of consecutive named code points.
    pub run_count: usize,
    /// The block shift the section was encoded with.
    pub block_shift: u8,
    /// The name layout the section was encoded with.
    pub layout: NameLayout,
    /// Header, page directory, runs and their padding.
    pub index_bytes: usize,
    /// The `u32` bit offset of every block (block-local layout only).
    pub offset_bytes: usize,
    /// The bits spent on in-stream checkpoint names (global only).
    #[allow(
        dead_code,
        reason = "reported by the layout study's tests, not read by `train`"
    )]
    pub checkpoint_bits: usize,
    /// The rounded byte cost of [`Self::checkpoint_bits`] (global only; part of `stream_bytes`).
    pub checkpoint_bytes: usize,
    /// The context map, model directory and model blobs.
    pub model_bytes: usize,
    /// The context map and model directory inside [`Self::model_bytes`].
    pub model_directory_bytes: usize,
    /// The Huffman bitstream.
    pub stream_bytes: usize,
    /// The exact bit length of [`Self::stream_bytes`] before its final-byte padding.
    pub stream_bits: usize,
    /// The number of byte contexts, including the start-of-name context.
    pub context_count: usize,
}

impl NameStats {
    /// The total section size in bytes.
    #[must_use]
    pub const fn total_bytes(&self) -> usize {
        // Checkpoint bits are part of the main stream, so they are reported separately but
        // must not be added here.
        self.index_bytes + self.offset_bytes + self.model_bytes + self.stream_bytes
    }
}

/// An encoded name section plus its size breakdown.
#[derive(Clone, Debug)]
pub struct Encoded {
    /// The complete section, ready for [`crate::nomina::image::pack`].
    pub bytes: Vec<u8>,
    /// The statistics and size breakdown of the section.
    pub stats: NameStats,
}

#[cfg(test)]
pub fn encode_with_block_shift(inputs: &Inputs, block_shift: u8) -> Encoded {
    encode_layout(inputs, block_shift, NameLayout::BlockLocal)
}

#[cfg(test)]
pub fn encode_global_with_block_shift(inputs: &Inputs, block_shift: u8) -> Encoded {
    encode_layout(inputs, block_shift, NameLayout::Global)
}

/// Encodes an already parsed corpus with the global checkpointed layout.
///
/// Training parses the corpus once and reuses it for every candidate block shift.
///
/// # Panics
///
/// Panics when `block_shift` is outside [`MIN_BLOCK_SHIFT`]..=[`MAX_BLOCK_SHIFT`], or when the
/// corpus exceeds the format's name-id or model-table limits.
#[must_use]
pub fn encode_global_corpus(corpus: &Corpus, block_shift: u8) -> Encoded {
    assert!(
        (MIN_BLOCK_SHIFT..=MAX_BLOCK_SHIFT).contains(&block_shift),
        "block shift {block_shift} is outside {MIN_BLOCK_SHIFT}..={MAX_BLOCK_SHIFT}"
    );
    encode_corpus(corpus, block_shift, NameLayout::Global)
}

/// Parses the inputs and encodes them with `layout`.
#[cfg(test)]
fn encode_layout(inputs: &Inputs, block_shift: u8, layout: NameLayout) -> Encoded {
    assert!(
        (MIN_BLOCK_SHIFT..=MAX_BLOCK_SHIFT).contains(&block_shift),
        "block shift {block_shift} is outside {MIN_BLOCK_SHIFT}..={MAX_BLOCK_SHIFT}"
    );
    let corpus = parse_corpus(inputs);
    encode_corpus(&corpus, block_shift, layout)
}

/// Encodes a parsed corpus with an explicit block shift and layout.
#[allow(
    clippy::too_many_lines,
    reason = "the frequency, encode and serialize passes are one linear pipeline"
)]
fn encode_corpus(corpus: &Corpus, block_shift: u8, layout: NameLayout) -> Encoded {
    let entries = &corpus.entries;
    assert!(!entries.is_empty(), "the corpus is empty");
    let global = layout == NameLayout::Global;
    let block_size = 1usize << block_shift;
    let block_count = entries.len().div_ceil(block_size);
    let (runs, run_count) = runs(entries);
    let page_dir = page_directory(&runs);

    // Pass 1: frequency tables. Context 0 is the start of a name; every other context is the
    // byte that precedes the next middle byte. The global layout also records the shared
    // suffix length of every name against its code-point predecessor.
    let mut prefix_freq = [0u64; 256];
    let mut suffix_freq = [0u64; 256];
    let mut checkpoint_freq = [0u64; 256];
    let mut context_freq = vec![[0u64; 256]; 256];
    for id in 0..entries.len() {
        let bytes = entries[id].1.as_bytes();
        let previous = predecessor(entries, id, block_size, global);
        let prefix = common_prefix(previous, bytes);
        prefix_freq[prefix] += 1;
        let suffix = if global {
            common_suffix(previous, bytes, prefix)
        } else {
            0
        };
        if global {
            suffix_freq[suffix] += 1;
        }
        let middle = bytes
            .get(prefix..bytes.len() - suffix)
            .expect("prefix and suffix are inside the name");
        let mut context = context_of(previous, prefix);
        for &byte in middle {
            context_freq[usize::from(context)][usize::from(byte)] += 1;
            context = byte;
        }
        context_freq[usize::from(context)][0] += 1;
    }
    if global {
        for id in (block_size..entries.len()).step_by(block_size) {
            for &byte in entries[id - 1].1.as_bytes() {
                checkpoint_freq[usize::from(byte)] += 1;
            }
            checkpoint_freq[0] += 1;
        }
        if checkpoint_freq.iter().all(|&count| count == 0) {
            // At most one block means no checkpoint is ever coded, but the runtime still
            // requires a non-empty model; seed one 1-bit symbol (the terminator) so the
            // serialized codebook has a maximum length. When checkpoints exist, `checkpoint_freq`
            // already counts their terminator bytes and this does not trigger.
            checkpoint_freq[0] = 1;
        }
    }

    let contexts: Vec<u8> = (0..=u8::MAX)
        .filter(|&context| {
            context_freq[usize::from(context)]
                .iter()
                .any(|&count| count > 0)
        })
        .collect();
    let context_base = if global { 2 } else { 1 };
    let model_slots = if global { 4 } else { 2 };
    assert!(
        u8::try_from(contexts.len() + model_slots).is_ok(),
        "too many byte contexts for one-byte model ids"
    );
    let generic = u8::try_from(context_base + contexts.len()).expect("context id fits in a byte");
    let mut context_model = [generic; 256];
    for (index, &context) in contexts.iter().enumerate() {
        context_model[usize::from(context)] =
            u8::try_from(context_base + index).expect("context model id fits in a byte");
    }

    // Pass 2: build the canonical codebooks. Model 0 is the prefix length; the global layout
    // adds the shared-suffix length at model 1; the byte contexts follow; the generic context
    // is last.
    let prefix_book = Codebook::new(&prefix_freq);
    let suffix_book = Codebook::new(&suffix_freq);
    let checkpoint_book = Codebook::new(&checkpoint_freq);
    let context_books: Vec<Codebook> = contexts
        .iter()
        .map(|&context| Codebook::new(&context_freq[usize::from(context)]))
        .collect();
    let generic_book = context_books
        .first()
        .cloned()
        .unwrap_or_else(|| Codebook::new(&[0u64; 256]));

    // Pass 3: encode one global MSB-first bitstream and record the block starts. The global
    // layout writes the predecessor as a full checkpoint entry at the start of every block
    // after the first; it is coded with the same models and costs about 16 bytes per block.
    let mut writer = BitWriter::new();
    let mut block_bits = Vec::with_capacity(block_count);
    let mut checkpoint_bits = 0usize;
    for id in 0..entries.len() {
        let bytes = entries[id].1.as_bytes();
        let previous = predecessor(entries, id, block_size, global);
        if id % block_size == 0 {
            block_bits.push(u32::try_from(writer.bit_len()).expect("bitstream fits in u32"));
            if global && id > 0 {
                let before = writer.bit_len();
                write_checkpoint(&mut writer, previous, &checkpoint_book);
                checkpoint_bits += writer.bit_len() - before;
            }
        }
        let prefix = common_prefix(previous, bytes);
        let suffix = if global {
            common_suffix(previous, bytes, prefix)
        } else {
            0
        };
        write_symbol(
            &mut writer,
            &prefix_book,
            u8::try_from(prefix).expect("prefix fits in a byte"),
        );
        if global {
            write_symbol(
                &mut writer,
                &suffix_book,
                u8::try_from(suffix).expect("suffix fits in a byte"),
            );
        }
        let middle = bytes
            .get(prefix..bytes.len() - suffix)
            .expect("prefix and suffix are inside the name");
        let mut context = context_of(previous, prefix);
        for &byte in middle {
            let model = model_of(
                &context_books,
                &generic_book,
                &context_model,
                context,
                context_base,
            );
            write_symbol(&mut writer, model, byte);
            context = byte;
        }
        let model = model_of(
            &context_books,
            &generic_book,
            &context_model,
            context,
            context_base,
        );
        write_symbol(&mut writer, model, 0);
    }

    // Serialize the header, directory and runs.
    let mut bytes = Vec::with_capacity(
        NAME_INDEX_HEADER_LEN
            + page_dir.len() * NAME_PAGE_DIRECTORY_ENTRY_LEN
            + run_count * NAME_RUN_LEN
            + block_count * 12
            + 6_000
            + writer.bit_len().div_ceil(8),
    );
    bytes.put_u32_le(u32::try_from(entries.len()).expect("name count fits in u32"));
    bytes.put_u32_le(u32::try_from(block_count).expect("block count fits in u32"));
    bytes.put_u16_le(u16::try_from(run_count).expect("run count fits in u16"));
    bytes.put_u16_le(u16::try_from(NAME_PAGE_COUNT).expect("page count fits in u16"));
    for &first in &page_dir {
        bytes.put_u16_le(first);
    }
    for &(first_cp, length, base_id) in &runs {
        bytes.put_u32_le(first_cp);
        bytes.put_u16_le(u16::try_from(length).expect("run length fits in u16"));
        bytes.put_u16_le(u16::try_from(base_id).expect("name id fits in u16"));
    }
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    let index_bytes = bytes.len();

    for &bit in &block_bits {
        bytes.put_u32_le(bit);
    }
    let offset_bytes = block_bits.len() * 4;

    // Model area: context map, directory, then blobs.
    let mut books: Vec<&Codebook> = vec![&prefix_book];
    if global {
        books.push(&suffix_book);
    }
    books.extend(&context_books);
    books.push(&generic_book);
    if global {
        books.push(&checkpoint_book);
    }
    let model_count = books.len();
    assert_model_limits(
        model_count,
        books.iter().map(|book| book.encoded_len()).sum(),
    );
    let model_start = bytes.len();
    bytes.extend_from_slice(&context_model);
    bytes.put_u16_le(u16::try_from(model_count).expect("model count fits in u16"));
    let directory_at = bytes.len();
    bytes.resize(
        directory_at + model_count * NAME_MODEL_DIRECTORY_ENTRY_LEN,
        0,
    );
    let blobs_at = bytes.len();
    let mut blob_offsets = Vec::with_capacity(model_count);
    for book in &books {
        let relative = bytes.len() - blobs_at;
        blob_offsets.push(u16::try_from(relative).expect("model offset fits in u16"));
        book.write(&mut bytes);
    }
    for (index, offset) in blob_offsets.iter().enumerate() {
        let at = directory_at + index * NAME_MODEL_DIRECTORY_ENTRY_LEN;
        (&mut bytes[at..at + 2]).put_u16_le(*offset);
    }
    let model_bytes = bytes.len() - model_start;
    let model_directory_bytes = blobs_at - model_start;

    let stream_bits = writer.bit_len();
    let stream = writer.finish();
    let stream_bytes = stream.len();
    bytes.extend_from_slice(&stream);

    Encoded {
        bytes,
        stats: NameStats {
            name_count: entries.len(),
            block_count,
            run_count,
            block_shift,
            layout,
            index_bytes,
            offset_bytes,
            checkpoint_bits,
            checkpoint_bytes: checkpoint_bits.div_ceil(8),
            model_bytes,
            model_directory_bytes,
            stream_bytes,
            stream_bits,
            context_count: contexts.len(),
        },
    }
}

/// Writes one in-stream checkpoint: its bytes use a dedicated order-0 model that is trained
/// on every checkpoint name, so it is guaranteed to contain every byte of the corpus.
fn write_checkpoint(writer: &mut BitWriter, name: &[u8], checkpoint_book: &Codebook) {
    for &byte in name {
        write_symbol(writer, checkpoint_book, byte);
    }
    write_symbol(writer, checkpoint_book, 0);
}

/// The predecessor a name is front-coded against.
#[allow(
    clippy::missing_const_for_fn,
    reason = "slice indexing is not allowed in a const fn"
)]
fn predecessor(entries: &[(u32, String)], id: usize, block_size: usize, global: bool) -> &[u8] {
    if id == 0 || (!global && id.is_multiple_of(block_size)) {
        &[]
    } else {
        entries[id - 1].1.as_bytes()
    }
}

/// The length of the common suffix of `previous` and `name`, after `prefix` shared bytes.
fn common_suffix(previous: &[u8], name: &[u8], prefix: usize) -> usize {
    let limit = (name.len() - prefix).min(previous.len().saturating_sub(prefix));
    let mut suffix = 0usize;
    while suffix < limit {
        let name_index = name.len() - 1 - suffix;
        let previous_index = previous.len() - 1 - suffix;
        if name[name_index] != previous[previous_index] {
            break;
        }
        suffix += 1;
    }
    suffix
}

/// The context of the first byte after `prefix` in a name.
#[allow(
    clippy::missing_const_for_fn,
    reason = "slice indexing is not allowed in a const fn"
)]
fn context_of(previous: &[u8], prefix: usize) -> u8 {
    if prefix == 0 { 0 } else { previous[prefix - 1] }
}

/// The codebook a previous-byte context selects.
fn model_of<'a>(
    context_books: &'a [Codebook],
    generic_book: &'a Codebook,
    context_model: &[u8; 256],
    context: u8,
    context_base: usize,
) -> &'a Codebook {
    let model = usize::from(context_model[usize::from(context)]);
    model
        .checked_sub(context_base)
        .and_then(|index| context_books.get(index))
        .unwrap_or(generic_book)
}

/// The length of the common prefix of `previous` and `name`.
fn common_prefix(previous: &[u8], name: &[u8]) -> usize {
    previous
        .iter()
        .zip(name)
        .take_while(|(left, right)| left == right)
        .count()
}

/// A maximal run of consecutive named code points: `(first_cp, length, first_name_id)`.
fn runs(entries: &[(u32, String)]) -> (Vec<(u32, u32, u32)>, usize) {
    let mut runs: Vec<(u32, u32, u32)> = Vec::with_capacity(720);
    let mut start = 0usize;
    for id in 1..=entries.len() {
        let ends = id == entries.len() || entries[id].0 != entries[id - 1].0 + 1;
        if ends {
            let length = u32::try_from(id - start).expect("run length fits in u32");
            runs.push((
                entries[start].0,
                length,
                u32::try_from(start).expect("name id fits in u32"),
            ));
            start = id;
        }
    }
    let count = runs.len();
    (runs, count)
}

/// `page_directory[page]` is the first run whose end is strictly after `page << NAME_PAGE_SHIFT`.
///
/// "Strictly" matters: a run ending exactly at the page base holds no code point of the page,
/// so it must be skipped. The runtime's run scan and index validation accept exactly this
/// directory.
fn page_directory(runs: &[(u32, u32, u32)]) -> Vec<u16> {
    let mut directory = Vec::with_capacity(NAME_PAGE_COUNT);
    let mut index = 0usize;
    for page in 0..NAME_PAGE_COUNT {
        let base = u32::try_from(page).expect("page fits in u32") << NAME_PAGE_SHIFT;
        while index < runs.len() && runs[index].0 + runs[index].1 <= base {
            index += 1;
        }
        directory.push(u16::try_from(index).expect("run index fits in u16"));
    }
    directory
}

/// A canonical Huffman codebook for one 256-symbol alphabet.
#[derive(Clone)]
struct Codebook {
    /// The longest code in the book.
    max_bits: u8,
    /// `counts[length]` is the number of symbols with that length (`counts[0] == 0`).
    counts: [u16; CODE_LENGTH_SLOTS],
    /// Symbols in canonical order.
    symbols: Vec<u8>,
    /// The canonical code per symbol.
    codes: Vec<Code>,
}

impl Codebook {
    /// Builds the canonical book for `frequencies`.
    fn new(frequencies: &[u64; 256]) -> Self {
        let lengths = code_lengths(frequencies, MAX_CODE_BITS);
        let mut counts = [0u16; CODE_LENGTH_SLOTS];
        for &length in &lengths {
            if length > 0 {
                counts[usize::from(length)] += 1;
            }
        }
        let mut pairs: Vec<(u8, u8)> = (0..=u8::MAX)
            .filter(|&symbol| lengths[usize::from(symbol)] > 0)
            .map(|symbol| (lengths[usize::from(symbol)], symbol))
            .collect();
        pairs.sort_unstable();
        let symbols: Vec<u8> = pairs.iter().map(|&(_, symbol)| symbol).collect();
        let max_bits = pairs.last().map_or(0, |&(length, _)| length);
        let codes = canonical_codes(&lengths);
        Self {
            max_bits,
            counts,
            symbols,
            codes,
        }
    }

    /// Appends one model blob: max bits, length counts, then the symbols.
    fn write(&self, out: &mut Vec<u8>) {
        out.push(self.max_bits);
        for length in 1..=usize::from(self.max_bits) {
            out.put_u16_le(self.counts[length]);
        }
        out.extend_from_slice(&self.symbols);
    }

    /// The number of bytes [`Self::write`] appends.
    fn encoded_len(&self) -> usize {
        1 + 2 * usize::from(self.max_bits) + self.symbols.len()
    }
}

/// Appends `symbol`'s canonical code to `writer`.
fn write_symbol(writer: &mut BitWriter, book: &Codebook, symbol: u8) {
    let code = book.codes[usize::from(symbol)];
    assert!(code.len > 0, "symbol {symbol} is absent from the model");
    writer.push(code.bits, usize::from(code.len));
}

/// The parsed name corpus.
pub struct Corpus {
    /// `(code point, name)`, ascending by code point.
    entries: Vec<(u32, String)>,
    /// The `*` pattern ranges, ascending by first code point.
    patterns: Vec<crate::nomina::patterns::Pattern>,
    /// Every Hangul record the file spells out, in file order.
    hangul: Vec<(u32, String)>,
    /// Aliases folded in from `NameAliases.txt`.
    #[allow(dead_code, reason = "used by the corpus statistics test")]
    aliases: usize,
}

impl Corpus {
    /// The `*` pattern ranges, sorted by start code point.
    #[must_use]
    pub fn patterns(&self) -> &[crate::nomina::patterns::Pattern] {
        &self.patterns
    }

    /// The Hangul name for `cp`, or `None` outside the syllable range.
    fn hangul_name(cp: u32) -> Option<String> {
        let mut name = String::new();
        write_hangul(cp, &mut name).then_some(name)
    }
}

/// Parses the name corpus from the vendored inputs.
///
/// This mirrors the current generator's `names::corpus` byte for byte, but is independent
/// of that private module: `DerivedName.txt` records, the `NameAliases.txt` fold with the
/// documented alias preference order, and the exclusion of Hangul and `*` patterns.
///
/// # Panics
///
/// Panics with the file and line when a record is malformed or a name is longer than
/// 255 bytes, when a code point appears twice, or when the corpus has more names than the
/// format's `u16` name ids can address. Malformed or oversized vendored data is a build
/// failure.
pub fn parse_corpus(inputs: &Inputs) -> Corpus {
    let mut entries: Vec<(u32, String)> = Vec::with_capacity(35_400);
    let mut hangul: Vec<(u32, String)> = Vec::with_capacity(HANGUL_COUNT);

    for (line, record) in crate::ucd::records(&inputs.names) {
        let source = Source {
            file: "DerivedName.txt",
            line,
        };
        let (lhs, rhs) = record
            .split_once(';')
            .unwrap_or_else(|| panic!("{source}: no field separator"));
        let lhs = lhs.trim();
        let name = rhs.trim();
        // Range records and the single-code-point `*` records are the pattern section's job;
        // `patterns::parse_patterns` validates and sorts them independently.
        if lhs.contains("..") || name.contains('*') {
            continue;
        }
        let cp = parse_cp(lhs, source);
        if (HANGUL_START..=HANGUL_END).contains(&cp) {
            assert!(
                name.starts_with("HANGUL SYLLABLE "),
                "{source}: unexpected Hangul name for U+{cp:04X}: {name}"
            );
            hangul.push((cp, name.to_owned()));
            continue;
        }
        assert!(
            name.len() <= 255,
            "{source}: name for U+{cp:04X} exceeds 255 bytes"
        );
        entries.push((cp, name.to_owned()));
    }

    let aliases = fold_aliases(&mut entries, &inputs.aliases);
    entries.sort_by_key(|(cp, _)| *cp);
    for pair in entries.windows(2) {
        assert!(
            pair[0].0 < pair[1].0,
            "duplicate code point U+{:04X} in the name corpus",
            pair[0].0
        );
    }
    assert_name_count(entries.len());
    Corpus {
        entries,
        patterns: crate::nomina::patterns::parse_patterns(&inputs.names),
        hangul,
        aliases,
    }
}

/// Asserts the corpus fits the format's `u16` name ids.
///
/// Run records store a `u16` base id, so the runtime cannot address a name whose id is over
/// `u16::MAX`. A larger corpus fails here with the observed count instead of the opaque
/// `expect` that serializing a run record would otherwise hit.
///
/// # Panics
///
/// Panics when `name_count` is over `u16::MAX`.
fn assert_name_count(name_count: usize) {
    assert!(
        u16::try_from(name_count).is_ok(),
        "the name corpus has {name_count} names; run records store u16 name ids, so the \
         format allows at most {}",
        u16::MAX
    );
}

/// Asserts the model table fits the format's model-id and `u16` offset fields.
///
/// The runtime rejects more than [`MAX_NAME_MODELS`] models and reads every model-directory
/// offset as a `u16` from the start of the model blobs.
///
/// # Panics
///
/// Panics with the observed count or size when either limit is exceeded.
fn assert_model_limits(model_count: usize, blob_bytes: usize) {
    assert!(
        model_count <= MAX_NAME_MODELS,
        "the name section needs {model_count} Huffman models; the format allows at most \
         {MAX_NAME_MODELS}"
    );
    assert!(
        u16::try_from(blob_bytes).is_ok(),
        "the model blobs are {blob_bytes} B; the format's u16 model offsets allow at most \
         {} B",
        u16::MAX
    );
}

/// The preference order of the alias kinds `NameAliases.txt` defines; the long `control` name
/// wins.
///
/// Returns `None` for a kind the file does not define, so a renamed or added type label fails
/// the build instead of silently reordering which alias text becomes the stored name.
fn alias_rank(kind: &str) -> Option<u8> {
    match kind {
        "control" => Some(0),
        "correction" => Some(1),
        "alternate" => Some(2),
        "figment" => Some(3),
        "abbreviation" => Some(4),
        _ => None,
    }
}

/// Adds `NameAliases.txt` names for code points that have none, keeping `entries` sorted.
///
/// Returns how many were added. Same tie-break as the current generator: aliases of the
/// same kind keep the first row in file order, so U+0009 answers `CHARACTER TABULATION`.
///
/// # Panics
///
/// Panics when a selected alias is over 255 bytes, the same bound the explicit names carry:
/// the runtime's 256-byte name buffer holds the name and its terminator. Panics too when an
/// alias kind is not one of the five type labels the file defines.
fn fold_aliases(entries: &mut Vec<(u32, String)>, aliases_text: &str) -> usize {
    let mut best: Vec<(u32, u8, &str, usize)> = Vec::new();
    for (line, record) in crate::ucd::records(aliases_text) {
        let source = Source {
            file: "NameAliases.txt",
            line,
        };
        let mut fields = record.split(';');
        let cp = parse_cp(fields.next().unwrap_or_default(), source);
        let alias = fields
            .next()
            .unwrap_or_else(|| panic!("{source}: missing alias"))
            .trim();
        let kind = fields
            .next()
            .unwrap_or_else(|| panic!("{source}: missing alias kind"))
            .trim();
        let rank =
            alias_rank(kind).unwrap_or_else(|| panic!("{source}: unknown alias kind {kind:?}"));
        match best
            .iter_mut()
            .find(|(candidate, _, _, _)| *candidate == cp)
        {
            Some(slot) if rank < slot.1 => {
                slot.1 = rank;
                slot.2 = alias;
                slot.3 = line;
            }
            Some(_) => {}
            None => best.push((cp, rank, alias, line)),
        }
    }
    best.sort_by_key(|(cp, _, _, _)| *cp);

    let mut added = 0usize;
    let mut cursor = 0usize;
    for (cp, _, alias, line) in best {
        while cursor < entries.len() && entries[cursor].0 < cp {
            cursor += 1;
        }
        if cursor < entries.len() && entries[cursor].0 == cp {
            continue;
        }
        assert!(
            alias.len() <= 255,
            "NameAliases.txt:{line}: alias for U+{cp:04X} exceeds 255 bytes"
        );
        entries.insert(cursor, (cp, alias.to_owned()));
        added += 1;
    }
    added
}

/// A UCD file name and 1-based physical line number, for diagnostics.
#[derive(Clone, Copy)]
struct Source<'a> {
    /// The file's `DerivedName`/`NameAliases` stem.
    file: &'a str,
    /// The 1-based physical line number.
    line: usize,
}

impl std::fmt::Display for Source<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.file, self.line)
    }
}

/// Parses one hexadecimal code point, naming its source on failure.
fn parse_cp(text: &str, source: Source<'_>) -> u32 {
    let text = text.trim();
    u32::from_str_radix(text, 16)
        .unwrap_or_else(|error| panic!("{source}: bad code point {text:?}: {error}"))
}

/// Decodes the produced name section and checks every name and every code-point mapping.
///
/// The checks are:
///
/// * every explicit name, decoded by id, is byte-identical to the corpus;
/// * every explicit code point resolves through the page directory and runs to its own id;
/// * every Hangul syllable reconstructed by the jamo tables is byte-identical to its
///   `DerivedName.txt` record;
/// * every `*` pattern range produces `prefix + hex(cp) + suffix` for every code point.
///
/// # Panics
///
/// Panics on the first mismatch, or when the section is malformed.
#[cfg(test)]
pub fn verify(section: &[u8], inputs: &Inputs) {
    verify_with_block_shift(section, inputs, NAME_BLOCK_SHIFT);
}

/// [`verify`] with an explicit block shift, for the block-size measurements.
///
/// # Panics
///
/// Panics on the first mismatch, or when the section is malformed.
#[cfg(test)]
pub fn verify_with_block_shift(section: &[u8], inputs: &Inputs, block_shift: u8) {
    verify_layout(section, inputs, block_shift, NameLayout::BlockLocal);
}

/// Verifies a global, checkpointed name section with an explicit block shift.
///
/// # Panics
///
/// Panics on the first mismatch, or when the section is malformed.
#[cfg(test)]
pub fn verify_global_with_block_shift(section: &[u8], inputs: &Inputs, block_shift: u8) {
    verify_layout(section, inputs, block_shift, NameLayout::Global);
}

/// [`verify`] with an explicit block shift and layout.
///
/// # Panics
///
/// Panics on the first mismatch, or when the section is malformed.
#[cfg(test)]
pub fn verify_layout(section: &[u8], inputs: &Inputs, block_shift: u8, layout: NameLayout) {
    let corpus = parse_corpus(inputs);
    verify_corpus(section, &corpus, block_shift, layout);
}

/// Verifies a global, checkpointed section against an already parsed corpus.
///
/// # Panics
///
/// Panics on the first mismatch, or when the section is malformed.
pub fn verify_global_corpus(section: &[u8], corpus: &Corpus, block_shift: u8) {
    verify_corpus(section, corpus, block_shift, NameLayout::Global);
}

/// The corpus-level checks shared by every verification entry point.
///
/// # Panics
///
/// Panics on the first mismatch, or when the section is malformed.
pub fn verify_corpus(section: &[u8], corpus: &Corpus, block_shift: u8, layout: NameLayout) {
    let mut decoded = Decoded::new(section, block_shift, layout);
    assert_eq!(
        decoded.name_count,
        corpus.entries.len(),
        "the section's name count"
    );

    for (id, (cp, name)) in corpus.entries.iter().enumerate() {
        assert_eq!(
            decoded.name_by_id(id).as_deref(),
            Some(name.as_str()),
            "name id {id} (U+{cp:04X})"
        );
        assert_eq!(decoded.entry_id(*cp), Some(id), "the id of U+{cp:04X}");
        assert_eq!(
            decoded.name(*cp).as_deref(),
            Some(name.as_str()),
            "the name of U+{cp:04X}"
        );
    }

    for cp in [0x0378u32, 0x10_FFFF, 0x11_0000, 0x11_0001] {
        assert_eq!(decoded.entry_id(cp), None, "U+{cp:04X} must be a miss");
    }

    for (cp, want) in &corpus.hangul {
        assert_eq!(
            Corpus::hangul_name(*cp).as_deref(),
            Some(want.as_str()),
            "the Hangul name of U+{cp:04X}"
        );
    }
    for pattern in &corpus.patterns {
        for cp in pattern.lo..=pattern.hi {
            let want = format!("{}{cp:04X}{}", pattern.prefix, pattern.suffix);
            assert_eq!(
                pattern_name(pattern, cp).as_deref(),
                Some(want.as_str()),
                "the pattern name of U+{cp:04X}"
            );
        }
    }
}

/// Builds a `*` pattern name the way the runtime does.
fn pattern_name(pattern: &crate::nomina::patterns::Pattern, cp: u32) -> Option<String> {
    if cp < pattern.lo || cp > pattern.hi {
        return None;
    }
    let mut name = String::new();
    write_pattern_name(&pattern.prefix, &pattern.suffix, cp, &mut name);
    Some(name)
}

/// A decoder used only by the build-time verifier. It is deliberately independent of the
/// runtime decoder so a format disagreement cannot pass verification.
struct Decoded<'a> {
    /// Number of names the section describes.
    name_count: usize,
    /// First bit of each block (the checkpoint entry for global blocks after the first).
    block_bits: &'a [u8],
    /// The name layout.
    layout: NameLayout,
    /// The name-block shift.
    shift: u32,
    /// The page directory.
    page_dir: &'a [u8],
    /// The run records.
    runs: &'a [u8],
    /// Number of runs.
    run_count: usize,
    /// Previous-byte to model-id map.
    context_model: [u8; 256],
    /// The canonical models, in id order.
    books: Vec<Book<'a>>,
    /// The MSB-first bitstream.
    stream: &'a [u8],
    /// Scratch space for the current chain.
    scratch: Vec<u8>,
    /// Scratch for the shared suffix, copied before the middle overwrites it.
    suffix_scratch: Vec<u8>,
    /// The id whose name is in `scratch` (`usize::MAX` for none).
    cache_id: usize,
    /// The bit position just after that entry.
    cache_bit: usize,
    /// The length of that entry's text.
    cache_len: usize,
}

/// One canonical Huffman model viewed as raw bytes.
struct Book<'a> {
    /// The longest code.
    max_bits: u8,
    /// `u16` counts by length, starting at length 1.
    counts: &'a [u8],
    /// Symbols in canonical order.
    symbols: &'a [u8],
}

impl<'a> Decoded<'a> {
    /// Parses and validates a name section.
    #[allow(
        clippy::too_many_lines,
        reason = "header, runs, checkpoints and models are one parse"
    )]
    fn new(section: &'a [u8], block_shift: u8, layout: NameLayout) -> Self {
        assert!(
            (MIN_BLOCK_SHIFT..=MAX_BLOCK_SHIFT).contains(&block_shift),
            "block shift {block_shift} is outside the supported range"
        );
        let mut header = section;
        let name_count =
            usize::try_from(header.try_get_u32_le().expect("name count: truncated u32"))
                .expect("name count fits");
        let block_count =
            usize::try_from(header.try_get_u32_le().expect("block count: truncated u32"))
                .expect("block count fits");
        let run_count = usize::from(header.try_get_u16_le().expect("run count: truncated u16"));
        let page_count = usize::from(header.try_get_u16_le().expect("page count: truncated u16"));
        assert_eq!(page_count, NAME_PAGE_COUNT, "the page directory size");
        let block_size = 1usize << block_shift;
        assert_eq!(
            block_count,
            name_count.div_ceil(block_size),
            "the block count"
        );

        let page_dir_at = NAME_INDEX_HEADER_LEN;
        let runs_at = page_dir_at + page_count * NAME_PAGE_DIRECTORY_ENTRY_LEN;
        let runs_end = runs_at + run_count * NAME_RUN_LEN;
        assert!(runs_end <= section.len(), "the run records are truncated");
        let starts_at = (runs_end + 3) & !3;
        let starts_end = starts_at + block_count * 4;
        assert!(
            starts_end <= section.len(),
            "the block starts are truncated"
        );

        let block_bits = &section[starts_at..starts_end];
        let models_at = starts_end;

        let context_model = section
            .get(models_at..models_at + NAME_CONTEXT_MAP_LEN)
            .expect("the context map is truncated");
        let count_at = models_at + NAME_CONTEXT_MAP_LEN;
        let mut count_bytes = section.get(count_at..).expect("model count: truncated u16");
        let model_count = usize::from(
            count_bytes
                .try_get_u16_le()
                .expect("model count: truncated u16"),
        );
        assert!(model_count > 0, "no models");
        let directory_at = models_at + NAME_CONTEXT_MAP_LEN + NAME_MODEL_COUNT_LEN;
        let blobs_at = directory_at + model_count * NAME_MODEL_DIRECTORY_ENTRY_LEN;
        assert!(
            blobs_at <= section.len(),
            "the model directory is truncated"
        );

        let mut books = Vec::with_capacity(model_count);
        let mut expected = 0usize;
        for index in 0..model_count {
            let offset_at = directory_at + index * NAME_MODEL_DIRECTORY_ENTRY_LEN;
            let mut offset_bytes = section
                .get(offset_at..)
                .expect("model offset: truncated u16");
            let relative = usize::from(
                offset_bytes
                    .try_get_u16_le()
                    .expect("model offset: truncated u16"),
            );
            assert_eq!(relative, expected, "model {index} is not contiguous");
            let at = blobs_at + relative;
            let max_bits = *section.get(at).expect("a model max-bits byte");
            assert!(
                max_bits > 0 && max_bits <= MAX_CODE_BITS,
                "model {index} has {max_bits} maximum bits"
            );
            let model_counts_at = at + 1;
            let symbols_at = model_counts_at + usize::from(max_bits) * 2;
            let mut symbols = 0usize;
            for length in 0..usize::from(max_bits) {
                let length_at = model_counts_at + length * 2;
                let mut length_bytes = section
                    .get(length_at..)
                    .expect("code counts: truncated u16");
                symbols += usize::from(
                    length_bytes
                        .try_get_u16_le()
                        .expect("code counts: truncated u16"),
                );
            }
            let symbols_end = symbols_at + symbols;
            assert!(symbols_end <= section.len(), "model {index} is truncated");
            books.push(Book {
                max_bits,
                counts: &section[model_counts_at..symbols_at],
                symbols: &section[symbols_at..symbols_end],
            });
            expected = symbols_end - blobs_at;
        }
        let stream_at = blobs_at + expected;
        assert!(stream_at <= section.len(), "the bitstream is truncated");
        let context_model = context_model
            .try_into()
            .expect("the context map is 256 bytes");

        Self {
            name_count,
            block_bits,
            layout,
            shift: u32::from(block_shift),
            page_dir: &section[page_dir_at..runs_at],
            runs: &section[runs_at..runs_end],
            run_count,
            context_model,
            books,
            stream: &section[stream_at..],
            scratch: vec![0; MAX_NAME_BYTES],
            suffix_scratch: vec![0; MAX_NAME_BYTES],
            cache_id: usize::MAX,
            cache_bit: 0,
            cache_len: 0,
        }
    }

    /// The id of `cp`, using the page directory and runs.
    fn entry_id(&self, cp: u32) -> Option<usize> {
        if cp > UNICODE_END {
            return None;
        }
        let page = usize::try_from(cp >> NAME_PAGE_SHIFT).expect("page fits in usize");
        let mut page_bytes = self
            .page_dir
            .get(page * 2..)
            .expect("page directory: truncated u16");
        let mut run = usize::from(
            page_bytes
                .try_get_u16_le()
                .expect("page directory: truncated u16"),
        );
        loop {
            if run >= self.run_count {
                return None;
            }
            let at = run * 8;
            let mut record = self.runs.get(at..).expect("run record: truncated");
            let first = record
                .try_get_u32_le()
                .expect("run first code point: truncated u32");
            if cp < first {
                return None;
            }
            let length = u32::from(record.try_get_u16_le().expect("run length: truncated u16"));
            if cp - first < length {
                let base = u32::from(record.try_get_u16_le().expect("run base id: truncated u16"));
                return Some(usize::try_from(base + (cp - first)).expect("id fits"));
            }
            run += 1;
        }
    }

    /// Decodes name `id`.
    fn name_by_id(&mut self, id: usize) -> Option<String> {
        if id >= self.name_count {
            return None;
        }
        if self.cache_id == id {
            return String::from_utf8(self.scratch[..self.cache_len].to_vec()).ok();
        }
        let block_size = 1usize << self.shift;
        let block = id >> self.shift;
        let global = self.layout == NameLayout::Global;

        let resume = self.cache_id != usize::MAX
            && id > self.cache_id
            && (id >> self.shift) == (self.cache_id >> self.shift);

        let (first, bit) = if resume {
            (self.cache_id + 1, self.cache_bit)
        } else {
            let bit_at = block * 4;
            let mut bit_bytes = self
                .block_bits
                .get(bit_at..)
                .expect("block bit offset: truncated u32");
            let bit = usize::try_from(
                bit_bytes
                    .try_get_u32_le()
                    .expect("block bit offset: truncated u32"),
            )
            .expect("bit offset fits in usize");
            (block * block_size, bit)
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
        String::from_utf8(self.scratch[..length].to_vec()).ok()
    }

    /// Decodes the in-stream checkpoint name of a global block.
    fn decode_checkpoint(&mut self, bits: &mut Bits<'_>) -> Option<usize> {
        let model = self.books.len().checked_sub(1)?;
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
        assert!(prefix <= length, "prefix length {prefix} exceeds the name");
        let suffix = if global {
            usize::from(self.symbol(1, bits)?)
        } else {
            0
        };
        assert!(
            suffix <= length - prefix,
            "suffix length {suffix} exceeds the previous name"
        );
        if suffix > 0 {
            let start = length - suffix;
            self.suffix_scratch[..suffix].copy_from_slice(&self.scratch[start..length]);
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

    /// The name of `cp`.
    fn name(&mut self, cp: u32) -> Option<String> {
        let id = self.entry_id(cp)?;
        self.name_by_id(id)
    }

    /// Decodes one canonical symbol.
    fn symbol(&self, model: usize, bits: &mut Bits<'_>) -> Option<u8> {
        let book = self.books.get(model)?;
        let mut code = 0u32;
        let mut first = 0u32;
        let mut index = 0usize;
        for length in 1..=book.max_bits {
            code = (code << 1) | u32::from(bits.bit()?);
            let at = usize::from(length - 1) * 2;
            let mut count_bytes = book.counts.get(at..)?;
            let count = u32::from(count_bytes.try_get_u16_le().ok()?);
            if code >= first && code - first < count {
                let offset = usize::try_from(code - first).expect("offset fits");
                return book.symbols.get(index + offset).copied();
            }
            index += usize::try_from(count).expect("count fits");
            first = (first + count) << 1;
        }
        None
    }
}

/// An MSB-first bit reader for the verifier.
struct Bits<'a> {
    /// The stream.
    data: &'a [u8],
    /// The absolute bit position.
    pos: usize,
    /// The maximum position, in bits.
    limit: usize,
}

impl<'a> Bits<'a> {
    /// A reader starting at `bit`.
    const fn new(data: &'a [u8], bit: usize) -> Self {
        Self {
            data,
            pos: bit,
            limit: data.len() * 8,
        }
    }

    /// Reads one bit.
    fn bit(&mut self) -> Option<u8> {
        if self.pos >= self.limit {
            return None;
        }
        let byte = *self.data.get(self.pos >> 3)?;
        let value = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    /// Reads the vendored inputs once per test.
    fn inputs() -> Inputs {
        let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../zeta-unicode/data");
        crate::ucd::read_inputs(&data).expect("vendored inputs")
    }

    /// A minimal corpus: two names and one `*` pattern range from a synthetic
    /// `DerivedName.txt`. Only `names` and `aliases` are parsed.
    fn minimal_inputs() -> Inputs {
        Inputs {
            names: "# DerivedName-18.0.0.txt\n\
                    0041; LATIN CAPITAL LETTER A\n\
                    0042; LATIN CAPITAL LETTER B\n\
                    3400..4DBF; CJK UNIFIED IDEOGRAPH-*\n"
                .to_owned(),
            gc: String::new(),
            aliases: String::new(),
            sequences: String::new(),
            emoji: String::new(),
            version: (18, 0, 0),
            emoji_version: (18, 0),
            paths: crate::ucd::InputPaths {
                names: PathBuf::from("DerivedName.txt"),
                gc: PathBuf::from("DerivedGeneralCategory.txt"),
                aliases: PathBuf::from("NameAliases.txt"),
                sequences: None,
                emoji: PathBuf::from("emoji-test.txt"),
            },
        }
    }

    /// A corpus smaller than one block has no checkpoints at all, but the global layout still
    /// serializes the checkpoint model; it must round-trip instead of writing an empty book.
    #[test]
    fn a_single_block_corpus_round_trips_in_the_global_layout() {
        let inputs = minimal_inputs();
        assert_eq!(parse_corpus(&inputs).entries.len(), 2);
        for shift in [1, 5] {
            // Two entries fit one 2^shift-name block: block_count == 1, no checkpoints.
            let encoded = encode_global_with_block_shift(&inputs, shift);
            assert_eq!(encoded.stats.block_count, 1, "block count at shift {shift}");
            assert_eq!(
                encoded.stats.checkpoint_bits, 0,
                "checkpoint bits at shift {shift}"
            );
            verify_global_with_block_shift(&encoded.bytes, &inputs, shift);
        }
    }

    #[test]
    fn corpus_matches_the_current_generator() {
        let corpus = parse_corpus(&inputs());
        assert_eq!(corpus.entries.len(), 35_416);
        assert_eq!(corpus.aliases, 65);
        assert_eq!(corpus.hangul.len(), HANGUL_COUNT);
        assert_eq!(corpus.patterns.len(), 20);
        assert_eq!(corpus.entries[0], (0x0000, "NULL".to_owned()));
        assert_eq!(
            corpus.entries.iter().find(|(cp, _)| *cp == 0x0009),
            Some(&(0x0009, "CHARACTER TABULATION".to_owned()))
        );
        assert_eq!(
            corpus.entries.iter().find(|(cp, _)| *cp == 0x0080),
            Some(&(0x0080, "PADDING CHARACTER".to_owned()))
        );
        assert_eq!(
            corpus.entries.last(),
            Some(&(0xE01EF, "VARIATION SELECTOR-256".to_owned()))
        );
        let (runs, count) = runs(&corpus.entries);
        assert_eq!(count, 719);
        assert_eq!(runs.iter().map(|run| run.1).max(), Some(1_812));
    }

    #[test]
    fn encodes_and_verifies_every_name() {
        let inputs = inputs();
        let encoded = encode_with_block_shift(&inputs, NAME_BLOCK_SHIFT);
        assert_eq!(encoded.stats.name_count, 35_416);
        assert_eq!(encoded.stats.block_count, 1_107);
        assert_eq!(encoded.stats.run_count, 719);
        assert_eq!(encoded.stats.context_count, 39);
        assert_eq!(encoded.stats.total_bytes(), encoded.bytes.len());
        eprintln!(
            "B=32: total {} = index {} + offsets {} + models {} (dir {}) + stream {} \
             ({} bits)",
            encoded.stats.total_bytes(),
            encoded.stats.index_bytes,
            encoded.stats.offset_bytes,
            encoded.stats.model_bytes,
            encoded.stats.model_directory_bytes,
            encoded.stats.stream_bytes,
            encoded.stats.stream_bits
        );
        // The section is the current design's measured size; this bound catches runaway
        // growth without pretending the 160 KB target is met.
        assert!(
            encoded.bytes.len() < 220_000,
            "section grew: {}",
            encoded.bytes.len()
        );
        verify(&encoded.bytes, &inputs);
    }

    #[test]
    fn every_block_shift_round_trips() {
        let inputs = inputs();
        for shift in [1, 2, 3, 4, 5, 6, 7, 8] {
            let encoded = encode_with_block_shift(&inputs, shift);
            verify_with_block_shift(&encoded.bytes, &inputs, shift);
            eprintln!(
                "block-local B={}: total {} (stream {})",
                1 << shift,
                encoded.bytes.len(),
                encoded.stats.stream_bytes
            );
        }
    }

    #[test]
    fn global_every_block_shift_round_trips() {
        let inputs = inputs();
        for shift in 3..=8 {
            let encoded = encode_global_with_block_shift(&inputs, shift);
            verify_global_with_block_shift(&encoded.bytes, &inputs, shift);
            assert_eq!(encoded.stats.layout, NameLayout::Global);
            assert_eq!(encoded.stats.total_bytes(), encoded.bytes.len());
            eprintln!(
                "global B={}: total {} = index {} + offsets {} + models {} + stream {} \
                 ({} bits, checkpoint {} bytes)",
                1 << shift,
                encoded.bytes.len(),
                encoded.stats.index_bytes,
                encoded.stats.offset_bytes,
                encoded.stats.model_bytes,
                encoded.stats.stream_bytes,
                encoded.stats.stream_bits,
                encoded.stats.checkpoint_bytes,
            );
        }
    }

    #[test]
    fn global_is_smaller_than_block_local_at_the_measured_shifts() {
        let inputs = inputs();
        for shift in 5..=8 {
            let local = encode_with_block_shift(&inputs, shift);
            let global = encode_global_with_block_shift(&inputs, shift);
            assert!(
                global.bytes.len() < local.bytes.len(),
                "global B={} is {} B, block-local is {} B",
                1 << shift,
                global.bytes.len(),
                local.bytes.len()
            );
        }
    }

    #[test]
    fn larger_blocks_are_smaller() {
        let inputs = inputs();
        let small = encode_with_block_shift(&inputs, 5);
        let medium = encode_with_block_shift(&inputs, 6);
        let large = encode_with_block_shift(&inputs, 7);
        assert!(medium.bytes.len() < small.bytes.len());
        assert!(large.bytes.len() < medium.bytes.len());
    }

    #[test]
    fn huffman_codes_are_prefix_free_and_round_trip() {
        for seed in 0..8u64 {
            let mut frequencies = [0u64; 256];
            for (symbol, slot) in frequencies.iter_mut().enumerate() {
                *slot = 1 + (u64::try_from(symbol).expect("symbol fits") * 37 + seed * 101) % 211;
            }
            let book = Codebook::new(&frequencies);
            assert!(book.max_bits <= MAX_CODE_BITS);
            // A complete canonical code satisfies Kraft's equality exactly. The encoder's
            // canonical assignment then makes the sorted codes strictly increasing at each
            // length, so the equality is both necessary and sufficient here.
            let kraft: u64 = book
                .counts
                .iter()
                .enumerate()
                .skip(1)
                .map(|(length, &count)| {
                    u64::from(count)
                        << (u32::from(MAX_CODE_BITS) - u32::try_from(length).expect("length"))
                })
                .sum();
            assert_eq!(kraft, 1 << MAX_CODE_BITS, "the code is not complete");
            for symbol in 0..=u8::MAX {
                let code = book.codes[usize::from(symbol)];
                if code.len > 0 {
                    assert!(
                        code.bits < (1u64 << code.len),
                        "symbol {symbol} does not fit its code length"
                    );
                }
            }
        }
    }

    #[test]
    fn a_skewed_distribution_is_bounded_to_the_maximum_depth() {
        let mut frequencies = [0u64; 256];
        let mut a = 1u64;
        let mut b = 1u64;
        for slot in frequencies.iter_mut().take(40) {
            *slot = a;
            let next = a.saturating_add(b);
            a = b;
            b = next;
        }
        let lengths = code_lengths(&frequencies, MAX_CODE_BITS);
        assert!(lengths.iter().all(|&length| length <= MAX_CODE_BITS));
        assert!(lengths.iter().any(|&length| length > 0));
    }

    #[test]
    fn page_directory_points_at_the_first_overlapping_run() {
        let runs = [(0x20u32, 1u32, 0u32), (0x100, 4, 1), (0x2000, 1, 5)];
        let directory = page_directory(&runs);
        assert_eq!(directory.len(), NAME_PAGE_COUNT);
        assert_eq!(directory[0], 0);
        assert_eq!(directory[0x2000 >> NAME_PAGE_SHIFT], 2);
        assert_eq!(directory[NAME_PAGE_COUNT - 1], 3);
    }

    #[test]
    fn huffman_symbols_survive_a_long_run_of_one_symbol() {
        let mut frequencies = [0u64; 256];
        frequencies[7] = 1_000_000;
        let book = Codebook::new(&frequencies);
        assert_eq!(book.max_bits, 1);
        assert_eq!(book.symbols, vec![7]);
    }

    #[test]
    fn patterns_are_computed_exactly() {
        let pattern = crate::nomina::patterns::Pattern {
            lo: 0x3400,
            hi: 0x3401,
            prefix: "CJK UNIFIED IDEOGRAPH-".to_owned(),
            suffix: String::new(),
        };
        assert_eq!(
            pattern_name(&pattern, 0x3400).as_deref(),
            Some("CJK UNIFIED IDEOGRAPH-3400")
        );
        assert_eq!(pattern_name(&pattern, 0x33FF), None);
    }

    #[test]
    fn alias_rank_prefers_the_long_control_name() {
        assert!(alias_rank("control") < alias_rank("abbreviation"));
        assert!(alias_rank("correction") < alias_rank("alternate"));
    }

    #[test]
    #[should_panic(expected = "NameAliases.txt:2: unknown alias kind \"renamed\"")]
    fn an_unknown_alias_kind_is_rejected() {
        let aliases = "# NameAliases-18.0.0.txt\n0041; LATINA; renamed\n";
        let mut entries = Vec::new();
        fold_aliases(&mut entries, aliases);
    }

    #[test]
    #[should_panic(expected = "NameAliases.txt:2: alias for U+0041 exceeds 255 bytes")]
    fn an_alias_over_255_bytes_is_rejected() {
        let name = "A".repeat(256);
        let aliases = format!("# NameAliases-18.0.0.txt\n0041; {name}; correction\n");
        let mut entries = Vec::new();
        fold_aliases(&mut entries, &aliases);
    }

    #[test]
    fn the_format_limits_accept_the_largest_encodable_values() {
        assert_name_count(usize::from(u16::MAX));
        assert_model_limits(MAX_NAME_MODELS, usize::from(u16::MAX));
    }

    #[test]
    #[should_panic(expected = "the name corpus has 65536 names")]
    fn too_many_names_are_rejected() {
        assert_name_count(usize::from(u16::MAX) + 1);
    }

    #[test]
    #[should_panic(expected = "the name section needs 256 Huffman models")]
    fn too_many_models_are_rejected() {
        assert_model_limits(MAX_NAME_MODELS + 1, 0);
    }

    #[test]
    #[should_panic(expected = "the model blobs are 65536 B")]
    fn model_blobs_that_overflow_u16_offsets_are_rejected() {
        assert_model_limits(1, usize::from(u16::MAX) + 1);
    }

    #[test]
    fn the_measured_codebook_length_matches_its_serialization() {
        let mut frequencies = [0u64; 256];
        for (symbol, slot) in frequencies.iter_mut().enumerate() {
            *slot = u64::try_from(symbol + 1).expect("symbol fits in u64");
        }
        let book = Codebook::new(&frequencies);
        let mut blob = Vec::new();
        book.write(&mut blob);
        assert_eq!(book.encoded_len(), blob.len());
    }
}
