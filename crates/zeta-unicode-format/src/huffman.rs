//! Canonical Huffman coding over an order-0 or order-1 byte model.
//!
//! The trainer and the runtime share this codec: the trainer emits the codebook and bit
//! payload, and the runtime decodes them. A stream starts with `u8 context_count`, then per
//! context `u8 context, u8 symbol_count` and a `(u8 symbol, u8 length)` pair per symbol,
//! followed by the MSB-first bit payload. Order 0 has one context; order 1 selects the next
//! model by the previous decoded byte.
//!
//! Keeping one implementation means an encoder change cannot silently drift from the decoder
//! the image is verified with.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// The longest code a model may use.
const MAX_BITS: usize = 32;

/// A canonical Huffman code: its bit pattern and length.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Code {
    /// The code's bits, right-aligned; only the low [`Self::len`] bits are meaningful.
    pub bits: u64,
    /// The number of bits in [`Self::bits`]; zero marks an absent symbol.
    pub len: u8,
}

/// Encodes `data` with an order-`order` canonical Huffman model.
///
/// # Panics
///
/// Panics when `order` is greater than one, when an order-1 corpus needs all 256 contexts (the
/// stream counts contexts in a `u8`, so 255 is the format's maximum; supporting 256 requires a
/// format change), when one context uses all 256 byte symbols (the stream counts symbols per
/// context in a `u8`, so 255 is the maximum), or when the codebook or payload does not fit the
/// section's `u16` lengths. Vendored Unicode data never does; a malformed call is a build
/// invariant violation.
#[must_use]
pub fn encode(data: &[u8], order: u8) -> Vec<u8> {
    assert!(order <= 1, "only order-0 and order-1 models exist");
    if data.is_empty() {
        return vec![0];
    }

    let mut contexts: Vec<u8> = Vec::new();
    let mut frequencies: Vec<[u32; 256]> = Vec::new();
    for (index, &byte) in data.iter().enumerate() {
        let context = context_at(data, index, order);
        let slot = match contexts.binary_search(&context) {
            Ok(slot) => slot,
            Err(slot) => {
                contexts.insert(slot, context);
                frequencies.insert(slot, [0u32; 256]);
                slot
            }
        };
        frequencies[slot][usize::from(byte)] += 1;
    }

    let models: Vec<Vec<(u8, u8)>> = frequencies.iter().map(lengths).collect();

    let mut out = Vec::new();
    assert!(
        contexts.len() <= 255,
        "an order-1 model has at most 255 contexts: the stream counts them in a u8"
    );
    out.push(u8::try_from(contexts.len()).expect("at most 255 contexts"));
    for (slot, &context) in contexts.iter().enumerate() {
        let model = &models[slot];
        assert!(
            model.len() <= 255,
            "a context with all 256 symbols needs a wider count than the u8 format has"
        );
        out.push(context);
        out.push(u8::try_from(model.len()).expect("at most 255 symbols"));
        for &(symbol, length) in model {
            out.push(symbol);
            out.push(length);
        }
    }

    let codes: Vec<Vec<Code>> = models
        .iter()
        .map(|model| {
            let mut table_lengths = [0u8; 256];
            for &(symbol, length) in model {
                table_lengths[usize::from(symbol)] = length;
            }
            canonical_codes(&table_lengths)
        })
        .collect();

    let mut bits = BitWriter::default();
    for (index, &byte) in data.iter().enumerate() {
        let context = context_at(data, index, order);
        let slot = contexts
            .binary_search(&context)
            .expect("the context was registered");
        let code = codes
            .get(slot)
            .and_then(|table| table.get(usize::from(byte)))
            .copied()
            .expect("the model covers every symbol");
        assert!(code.len != 0, "every symbol has a code");
        bits.push(code.bits, usize::from(code.len));
    }
    out.extend_from_slice(&bits.finish());
    assert!(
        u16::try_from(out.len()).is_ok(),
        "the Huffman stream is over u16 bytes"
    );
    out
}

/// The model-selecting context of `data[index]`: the previous byte for order 1, otherwise the
/// start-of-stream context `0`.
const fn context_at(data: &[u8], index: usize, order: u8) -> u8 {
    if order == 0 || index == 0 {
        0
    } else {
        data[index - 1]
    }
}

/// Decodes a stream built by [`encode`].
///
/// Returns `None` for an order above one, a `raw_len` larger than the stream could encode (every
/// symbol consumes at least one bit), a truncated codebook, an invalid or oversubscribed model,
/// or a payload shorter than `raw_len`.
#[must_use]
pub fn decode(stream: &[u8], raw_len: usize, order: u8) -> Option<Vec<u8>> {
    if order > 1 {
        return None;
    }
    if raw_len > stream.len().saturating_mul(8) {
        return None;
    }
    if raw_len == 0 {
        return Some(Vec::new());
    }
    let mut at = 0usize;
    let context_count = usize::from(*stream.get(at)?);
    at += 1;
    let mut models: Vec<Option<Model>> = Vec::new();
    models.resize_with(256, || None);
    for _ in 0..context_count {
        let context = usize::from(*stream.get(at)?);
        at += 1;
        let symbol_count = usize::from(*stream.get(at)?);
        at += 1;
        let mut pairs = Vec::with_capacity(symbol_count);
        for _ in 0..symbol_count {
            let symbol = *stream.get(at)?;
            at += 1;
            let length = usize::from(*stream.get(at)?);
            at += 1;
            if length == 0 || length > MAX_BITS {
                return None;
            }
            pairs.push((symbol, length));
        }
        *models.get_mut(context)? = Some(Model::new(pairs)?);
    }

    let mut reader = BitReader::new(stream.get(at..)?);
    let mut out = Vec::with_capacity(raw_len);
    let mut previous = 0u8;
    for _ in 0..raw_len {
        let context = if order == 0 { 0 } else { previous };
        let symbol = models
            .get(usize::from(context))?
            .as_ref()?
            .symbol(&mut reader)?;
        out.push(symbol);
        previous = symbol;
    }
    Some(out)
}

/// A canonical Huffman model: bit-length counts and symbols in canonical order.
struct Model {
    counts: [u16; MAX_BITS + 1],
    symbols: Vec<u8>,
    max_bits: usize,
}

impl Model {
    /// Builds the decoder model from `(symbol, length)` pairs.
    ///
    /// Returns `None` for a length outside the codec's limit or a codebook that is empty or
    /// oversubscribed. The runtime's general-category and emoji parsers decode streams without
    /// a separate codebook check, so validation has to happen here.
    fn new(pairs: Vec<(u8, usize)>) -> Option<Self> {
        let mut counts = [0u16; MAX_BITS + 1];
        let mut max_bits = 0usize;
        for &(_, length) in &pairs {
            *counts.get_mut(length)? += 1;
            max_bits = max_bits.max(length);
        }
        if !valid_codebook(&counts, u8::try_from(max_bits).ok()?) {
            return None;
        }
        let mut ordered = pairs;
        ordered.sort_unstable_by_key(|&(symbol, length)| (length, symbol));
        let symbols = ordered.into_iter().map(|(symbol, _)| symbol).collect();
        Some(Self {
            counts,
            symbols,
            max_bits,
        })
    }

    /// Decodes one symbol.
    fn symbol(&self, reader: &mut BitReader<'_>) -> Option<u8> {
        let mut code = 0usize;
        let mut first = 0usize;
        let mut index = 0usize;
        for length in 1..=self.max_bits {
            code = (code << 1) | usize::from(reader.bit()?);
            let count = usize::from(*self.counts.get(length)?);
            if code >= first && code - first < count {
                return self.symbols.get(index + code - first).copied();
            }
            index += count;
            first = (first + count) << 1;
        }
        None
    }
}

/// Builds the code lengths of a canonical Huffman model from `frequencies`.
fn lengths(frequencies: &[u32; 256]) -> Vec<(u8, u8)> {
    let mut wide = [0u64; 256];
    for (slot, &frequency) in wide.iter_mut().zip(frequencies) {
        *slot = u64::from(frequency);
    }
    let lengths = code_lengths(
        &wide,
        u8::try_from(MAX_BITS).expect("MAX_BITS fits in a byte"),
    );
    (0..=u8::MAX)
        .filter(|&symbol| lengths[usize::from(symbol)] > 0)
        .map(|symbol| (symbol, lengths[usize::from(symbol)]))
        .collect()
}

/// Length-limited canonical Huffman code lengths for a 256-symbol alphabet.
///
/// The tree is built at full precision; when it is deeper than `max_bits`, every frequency is
/// halved (never below one) and the tree is rebuilt. Flattening the distribution this way
/// bounds the depth without ever changing the prefix property. A one-symbol alphabet gets a
/// one-bit code; an empty alphabet gets no codes.
///
/// # Panics
///
/// Panics if `max_bits` is zero, if the alphabet is wider than `1 << max_bits` (no
/// length-limited code can represent it), if a tree needs a code longer than 255 bits (the
/// reported depth is a `u8`) or if the frequency scale overflows `u64`; once the alphabet fits
/// the code space, flattening to unit weights bounds the depth, so the last two are
/// unreachable.
#[must_use]
pub fn code_lengths(frequencies: &[u64; 256], max_bits: u8) -> [u8; 256] {
    assert!(max_bits > 0, "a codebook needs at least one bit");
    let mut lengths = [0u8; 256];
    let symbols: Vec<u8> = (0..=u8::MAX)
        .filter(|&symbol| frequencies[usize::from(symbol)] > 0)
        .collect();
    // Doubling `scale` can never flatten an alphabet wider than the code space, so reject it
    // before the retry loop misdiagnoses it as a `u64` overflow.
    let capacity = 1u64.checked_shl(u32::from(max_bits)).unwrap_or(u64::MAX);
    let symbol_count = u64::try_from(symbols.len()).expect("at most 256 symbols");
    assert!(
        symbol_count <= capacity,
        "cannot fit {} symbols into codes of at most {max_bits} bits",
        symbols.len()
    );
    match symbols.len() {
        0 => return lengths,
        1 => {
            lengths[usize::from(symbols[0])] = 1;
            return lengths;
        }
        _ => {}
    }

    let mut scale = 1u64;
    loop {
        let mut nodes: Vec<Node> = Vec::with_capacity(symbols.len() * 2);
        for &symbol in &symbols {
            nodes.push(Node::leaf(symbol));
        }
        let mut heap: BinaryHeap<Reverse<(u64, usize)>> = symbols
            .iter()
            .enumerate()
            .map(|(index, &symbol)| {
                let weight = (frequencies[usize::from(symbol)] / scale).max(1);
                Reverse((weight, index))
            })
            .collect();
        let mut serial = nodes.len();
        while heap.len() > 1 {
            let Some(Reverse((left_weight, left))) = heap.pop() else {
                break;
            };
            let Some(Reverse((right_weight, right))) = heap.pop() else {
                break;
            };
            nodes.push(Node::branch(left, right));
            heap.push(Reverse((left_weight + right_weight, serial)));
            serial += 1;
        }
        let Some(Reverse((_, root))) = heap.pop() else {
            return lengths;
        };
        let mut max_depth = 0u16;
        let mut stack = vec![(root, 0u16)];
        while let Some((index, level)) = stack.pop() {
            let Some(node) = nodes.get(index) else {
                continue;
            };
            if let Some(symbol) = node.symbol {
                lengths[usize::from(symbol)] =
                    u8::try_from(level).expect("tree depth fits in a byte");
                max_depth = max_depth.max(level);
                continue;
            }
            let child = level + 1;
            stack.push((node.left, child));
            stack.push((node.right, child));
        }
        if max_depth <= u16::from(max_bits) {
            return lengths;
        }
        scale = scale.checked_mul(2).expect("frequency scale fits in u64");
    }
}

/// One node of the Huffman tree.
struct Node {
    /// The leaf's symbol, or `None` for a branch.
    symbol: Option<u8>,
    /// The left child of a branch.
    left: usize,
    /// The right child of a branch.
    right: usize,
}

impl Node {
    /// A leaf node.
    const fn leaf(symbol: u8) -> Self {
        Self {
            symbol: Some(symbol),
            left: 0,
            right: 0,
        }
    }

    /// A branch node.
    const fn branch(left: usize, right: usize) -> Self {
        Self {
            symbol: None,
            left,
            right,
        }
    }
}

/// Assigns canonical codes to a set of code lengths.
///
/// `lengths[symbol]` is the code length of that symbol; zero marks an absent symbol. Lengths are
/// limited to the codec's 32-bit maximum so the canonical increment cannot overflow `u64`. The
/// returned table is indexed by symbol, with zeroed entries for absent symbols.
///
/// # Panics
///
/// Panics if a nonzero length exceeds 32 bits.
#[must_use]
pub fn canonical_codes(lengths: &[u8]) -> Vec<Code> {
    assert!(
        lengths
            .iter()
            .all(|&length| usize::from(length) <= MAX_BITS),
        "a canonical code is at most {MAX_BITS} bits long"
    );
    let mut ordered: Vec<(u8, usize)> = lengths
        .iter()
        .enumerate()
        .filter(|&(_, &length)| length != 0)
        .map(|(symbol, &length)| (length, symbol))
        .collect();
    ordered.sort_unstable();
    let mut codes = vec![Code::default(); lengths.len()];
    let mut bits = 0u64;
    let mut previous = 0u8;
    for (len, symbol) in ordered {
        bits <<= u32::from(len - previous);
        codes[symbol] = Code { bits, len };
        bits += 1;
        previous = len;
    }
    codes
}

/// Is `counts` a non-oversubscribed canonical codebook with `max_bits` maximum length?
///
/// `counts[length]` is the number of symbols with that length. A single-symbol model is valid:
/// its one-bit code is a complete code for the one-symbol alphabet. `max_bits` is limited to
/// the codec's 32-bit maximum so the Kraft sum cannot overflow.
///
/// # Panics
///
/// Panics if `max_bits` is greater than 32.
#[must_use]
pub fn valid_codebook(counts: &[u16], max_bits: u8) -> bool {
    assert!(
        usize::from(max_bits) <= MAX_BITS,
        "a codebook has at most {MAX_BITS} bits"
    );
    let max = usize::from(max_bits);
    if max == 0 || counts.len() <= max {
        return false;
    }
    let mut kraft = 0u64;
    for length in 1..=max {
        let Some(&count) = counts.get(length) else {
            return false;
        };
        kraft += u64::from(count) << (max - length);
    }
    kraft <= 1u64 << max
}

/// An MSB-first bit writer.
#[derive(Default)]
pub struct BitWriter {
    out: Vec<u8>,
    accumulator: u64,
    bits: usize,
}

impl BitWriter {
    /// An empty writer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            out: Vec::new(),
            accumulator: 0,
            bits: 0,
        }
    }

    /// The number of bits written so far, including bits still pending in the last byte.
    #[must_use]
    pub const fn bit_len(&self) -> usize {
        self.out.len() * 8 + self.bits
    }

    /// Appends the low `len` bits of `value`, most significant first.
    ///
    /// # Panics
    ///
    /// Panics when `len` is greater than 64.
    pub fn push(&mut self, value: u64, len: usize) {
        assert!(len <= 64, "at most 64 bits fit one push");
        let mut remaining = len;
        while remaining > 0 {
            // The accumulator holds fewer than eight pending bits, so a 56-bit chunk always
            // leaves room to shift it into the accumulator.
            let take = remaining.min(56);
            let shift = remaining - take;
            let chunk = (value >> shift) & ((1u64 << take) - 1);
            self.accumulator = (self.accumulator << take) | chunk;
            self.bits += take;
            while self.bits >= 8 {
                #[allow(clippy::cast_possible_truncation, reason = "the top byte")]
                self.out.push((self.accumulator >> (self.bits - 8)) as u8);
                self.bits -= 8;
            }
            self.accumulator &= (1u64 << self.bits) - 1;
            remaining -= take;
        }
    }

    /// Pads the final byte with zeros and returns the written bytes.
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        if self.bits != 0 {
            #[allow(clippy::cast_possible_truncation, reason = "the padded byte")]
            self.out.push((self.accumulator << (8 - self.bits)) as u8);
        }
        self.out
    }
}

/// An MSB-first bit reader.
struct BitReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> BitReader<'a> {
    /// Starts at the first bit of `data`.
    const fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    /// Reads one bit.
    fn bit(&mut self) -> Option<u8> {
        let byte = self.data.get(self.position >> 3)?;
        let bit = (byte >> (7 - (self.position & 7))) & 1;
        self.position += 1;
        Some(bit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_round_trip_for_both_orders() {
        for (data, order) in [
            (b"".as_slice(), 0u8),
            (b"x".as_slice(), 0),
            (b"aaaaabbbbcc".as_slice(), 0),
            (b"abracadabra".as_slice(), 1),
            (b"the quick brown fox jumps over the lazy dog".as_slice(), 1),
        ] {
            let stream = encode(data, order);
            assert_eq!(decode(&stream, data.len(), order).as_deref(), Some(data));
        }
    }

    #[test]
    fn generated_streams_round_trip_at_both_orders() {
        // A deterministic LCG keeps the test dependency-free while covering skewed alphabets,
        // every context byte and payloads that cross a byte boundary.
        let mut state = 0x9E37_79B9u32;
        for (length, order) in [(1usize, 0u8), (7, 1), (256, 0), (4_096, 1)] {
            let data: Vec<u8> = (0..length)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    // A 32-symbol alphabet keeps every context well under the u8 symbol count
                    // while still exercising several order-1 models.
                    u8::try_from((state >> 24) % 32).expect("a small symbol")
                })
                .collect();
            let stream = encode(&data, order);
            assert_eq!(
                decode(&stream, data.len(), order).as_deref(),
                Some(data.as_slice()),
                "length {length}, order {order}"
            );
        }
    }

    #[test]
    fn a_skewed_alphabet_round_trips_in_both_orders() {
        let mut data = vec![0u8; 2_000];
        for (index, byte) in data.iter_mut().enumerate() {
            *byte = match index % 9 {
                0 => 0,
                1 => 255,
                other => u8::try_from(other).expect("a small value"),
            };
        }
        for order in [0, 1] {
            let stream = encode(&data, order);
            assert_eq!(
                decode(&stream, data.len(), order).as_deref(),
                Some(&data[..])
            );
        }
    }

    #[test]
    #[should_panic(expected = "an order-1 model has at most 255 contexts")]
    fn a_corpus_needing_256_contexts_is_rejected() {
        // `0..=255` twice gives the first byte the start context and every later byte its
        // predecessor as context, so all 256 byte values become contexts.
        let data: Vec<u8> = (0..=255u8).chain(0..=255).collect();
        let _ = encode(&data, 1);
    }

    #[test]
    fn truncated_streams_are_rejected() {
        let data = b"abracadabra";
        let stream = encode(data, 1);
        for length in 0..stream.len() {
            assert_eq!(decode(&stream[..length], data.len(), 1), None);
        }
    }

    #[test]
    fn decode_rejects_an_unknown_order_before_the_empty_shortcut() {
        assert_eq!(decode(&[], 0, 2), None);
    }

    #[test]
    fn decode_rejects_an_impossible_raw_len_without_allocating() {
        // Every symbol consumes at least one bit, so a one-byte stream cannot hold nine bytes.
        assert_eq!(decode(&[0], 9, 0), None);
        assert_eq!(decode(&[0], usize::MAX, 0), None);
    }

    #[test]
    fn decode_rejects_an_oversubscribed_codebook() {
        // Three one-bit codes in one context: 3/2 > 1, so the old decoder silently returned
        // "aaaa" from the zero payload.
        let stream = [1, 0, 3, b'a', 1, b'b', 1, b'c', 1, 0b0000_0000];
        assert_eq!(decode(&stream, 4, 0), None);
    }

    #[test]
    fn codebooks_are_validated_for_oversubscription() {
        let mut single = [0u16; 17];
        single[1] = 1;
        assert!(valid_codebook(&single, 1));

        let mut complete = [0u16; 17];
        complete[1] = 1;
        complete[2] = 2;
        assert!(valid_codebook(&complete, 2));

        let mut oversubscribed = [0u16; 17];
        oversubscribed[1] = 3;
        assert!(!valid_codebook(&oversubscribed, 2));

        let empty = [0u16; 17];
        assert!(
            valid_codebook(&empty, 1),
            "an unused model is not oversubscribed"
        );
        assert!(!valid_codebook(&complete, 0));
        assert!(!valid_codebook(&[0; 2], 2), "counts must cover max_bits");
    }

    #[test]
    #[should_panic(expected = "cannot fit 256 symbols")]
    fn code_lengths_rejects_an_alphabet_wider_than_the_code_space() {
        let _ = code_lengths(&[1; 256], 7);
    }

    #[test]
    #[should_panic(expected = "a canonical code is at most 32 bits long")]
    fn canonical_codes_rejects_codes_beyond_the_codec_limit() {
        let _ = canonical_codes(&[33]);
    }

    #[test]
    #[should_panic(expected = "a codebook has at most 32 bits")]
    fn valid_codebook_rejects_an_out_of_range_limit() {
        let _ = valid_codebook(&[0; 17], 33);
    }

    #[test]
    fn canonical_codes_assign_shortlex_codes() {
        let codes = canonical_codes(&[1, 2, 3, 3]);
        assert_eq!(codes[0], Code { bits: 0b0, len: 1 });
        assert_eq!(codes[1], Code { bits: 0b10, len: 2 });
        assert_eq!(
            codes[2],
            Code {
                bits: 0b110,
                len: 3
            }
        );
        assert_eq!(
            codes[3],
            Code {
                bits: 0b111,
                len: 3
            }
        );
    }

    #[test]
    fn bit_writer_packs_msb_first_and_handles_wide_pushes() {
        let mut writer = BitWriter::new();
        writer.push(0b1, 1);
        writer.push(0b01, 2);
        writer.push(0b101, 3);
        assert_eq!(writer.bit_len(), 6);
        assert_eq!(writer.finish(), vec![0b1011_0100]);

        let mut writer = BitWriter::new();
        writer.push(1, 1);
        writer.push(u64::MAX, 64);
        assert_eq!(writer.bit_len(), 65);
        let mut expected = vec![0xFF; 8];
        expected.push(0x80);
        assert_eq!(writer.finish(), expected);
    }
}
