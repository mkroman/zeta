//! The Nomina runtime: one flat image, zero-copy lookups.
//!
//! [`Nomina`] parses the 64-byte image header once and reads the four sections. Stored
//! names decode from a single front-coded block with a fixed 256-byte scratch buffer; Hangul and
//! `*` pattern names are composed on the spot. The emoji (`emoji`) and general-category
//! (`gc`) sections are decoded by their own modules.
//!
//! The image is built by `zeta_unicode_build::nomina`: section 0 is [`names`], section 1 is
//! `emoji`, section 2 is `gc` and section 3 is the pattern table parsed by [`image`]. [`crate`]
//! wraps one image in a thread-local name decoder and process-wide emoji and category tables.

pub mod emoji;
pub mod gc;
pub mod image;
pub mod names;

pub use image::Nomina;

#[cfg(test)]
pub mod test_support {
    /// A minimal flat Huffman encoder for tests: every symbol gets an eight-bit code.
    pub fn flat_stream(data: &[u8], order: u8) -> Vec<u8> {
        let context_at = |index: usize| -> u8 {
            if order == 0 || index == 0 {
                0
            } else {
                data[index - 1]
            }
        };
        let mut contexts: Vec<u8> = Vec::new();
        for index in 0..data.len() {
            let context = context_at(index);
            if let Err(slot) = contexts.binary_search(&context) {
                contexts.insert(slot, context);
            }
        }
        let mut out = vec![u8::try_from(contexts.len()).unwrap()];
        let mut maps: Vec<Vec<(u8, u8)>> = Vec::new();
        for &context in &contexts {
            let mut symbols: Vec<u8> = data
                .iter()
                .enumerate()
                .filter(|&(index, _)| context_at(index) == context)
                .map(|(_, &byte)| byte)
                .collect();
            symbols.sort_unstable();
            symbols.dedup();
            out.push(context);
            out.push(u8::try_from(symbols.len()).unwrap());
            let mut map = Vec::new();
            for (code, &symbol) in symbols.iter().enumerate() {
                out.push(symbol);
                out.push(8);
                map.push((symbol, u8::try_from(code).unwrap()));
            }
            maps.push(map);
        }
        let mut bits = Vec::new();
        let mut accumulator = 0u8;
        let mut count = 0u8;
        for (index, &byte) in data.iter().enumerate() {
            let context = context_at(index);
            let slot = contexts.binary_search(&context).unwrap();
            let code = maps[slot]
                .iter()
                .find(|&&(symbol, _)| symbol == byte)
                .unwrap()
                .1;
            for bit in (0..8).rev() {
                accumulator = (accumulator << 1) | ((code >> bit) & 1);
                count += 1;
                if count == 8 {
                    bits.push(accumulator);
                    accumulator = 0;
                    count = 0;
                }
            }
        }
        if count != 0 {
            bits.push(accumulator << (8 - count));
        }
        out.extend_from_slice(&bits);
        out
    }
}
