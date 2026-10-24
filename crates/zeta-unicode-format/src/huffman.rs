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
/// Panics when `order` is greater than one, or when the codebook or payload does not fit the
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
        let context = if order == 0 || index == 0 {
            0
        } else {
            data[index - 1]
        };
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
    out.push(u8::try_from(contexts.len()).expect("at most 256 contexts"));
    assert!(
        contexts.len() <= 256,
        "an order-1 model has at most 256 contexts"
    );
    for (slot, &context) in contexts.iter().enumerate() {
        let model = &models[slot];
        out.push(context);
        out.push(u8::try_from(model.len()).expect("at most 256 symbols"));
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
        let context = if order == 0 || index == 0 {
            0
        } else {
            data[index - 1]
        };
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

/// Decodes a stream built by [`encode`].
///
/// Returns `None` for a truncated codebook, an invalid code, or a payload shorter than
/// `raw_len`.
#[must_use]
pub fn decode(stream: &[u8], raw_len: usize, order: u8) -> Option<Vec<u8>> {
    if raw_len == 0 {
        return Some(Vec::new());
    }
    if order > 1 {
        return None;
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
    fn new(pairs: Vec<(u8, usize)>) -> Option<Self> {
        let mut counts = [0u16; MAX_BITS + 1];
        let mut max_bits = 0usize;
        for &(_, length) in &pairs {
            *counts.get_mut(length)? += 1;
            max_bits = max_bits.max(length);
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

/// One node of the Huffman tree.
struct Node {
    left: Option<usize>,
    right: Option<usize>,
    symbol: Option<u8>,
}

/// Builds the code lengths of a canonical Huffman model from `frequencies`.
fn lengths(frequencies: &[u32; 256]) -> Vec<(u8, u8)> {
    let used: Vec<u8> = frequencies
        .iter()
        .enumerate()
        .filter(|&(_, &frequency)| frequency != 0)
        .map(|(symbol, _)| u8::try_from(symbol).expect("a byte symbol"))
        .collect();
    match used.len() {
        0 => Vec::new(),
        1 => vec![(used[0], 1)],
        _ => {
            let mut heap = BinaryHeap::new();
            let mut nodes: Vec<Node> = Vec::new();
            for &symbol in &used {
                let frequency = u64::from(frequencies[usize::from(symbol)]);
                nodes.push(Node {
                    left: None,
                    right: None,
                    symbol: Some(symbol),
                });
                heap.push(Reverse((frequency, nodes.len() - 1)));
            }
            while heap.len() > 1 {
                let Some(Reverse((left_frequency, left))) = heap.pop() else {
                    break;
                };
                let Some(Reverse((right_frequency, right))) = heap.pop() else {
                    break;
                };
                nodes.push(Node {
                    left: Some(left),
                    right: Some(right),
                    symbol: None,
                });
                heap.push(Reverse((left_frequency + right_frequency, nodes.len() - 1)));
            }
            let Some(Reverse((_, root))) = heap.pop() else {
                return Vec::new();
            };
            let mut depths = vec![0u8; 256];
            let mut stack = vec![(root, 0u8)];
            while let Some((index, depth)) = stack.pop() {
                let Some(node) = nodes.get(index) else {
                    continue;
                };
                if let Some(symbol) = node.symbol {
                    assert!(
                        usize::from(depth) <= MAX_BITS,
                        "Huffman depth {depth} exceeds {MAX_BITS}"
                    );
                    depths[usize::from(symbol)] = depth;
                    continue;
                }
                if let (Some(left), Some(right)) = (node.left, node.right) {
                    stack.push((left, depth + 1));
                    stack.push((right, depth + 1));
                }
            }
            let mut out: Vec<(u8, u8)> = used
                .into_iter()
                .map(|symbol| {
                    let depth = depths[usize::from(symbol)];
                    (symbol, depth)
                })
                .collect();
            out.sort_unstable();
            out
        }
    }
}

/// Assigns canonical codes to a set of code lengths.
///
/// `lengths[symbol]` is the code length of that symbol; zero marks an absent symbol. The
/// returned table is indexed by symbol, with zeroed entries for absent symbols.
#[must_use]
pub fn canonical_codes(lengths: &[u8]) -> Vec<Code> {
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
    fn truncated_streams_are_rejected() {
        let data = b"abracadabra";
        let stream = encode(data, 1);
        for length in 0..stream.len() {
            assert_eq!(decode(&stream[..length], data.len(), 1), None);
        }
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
