//! Byte primitives shared by the image trainer and the runtime.
//!
//! Every integer the Nomina format writes is little-endian except the LEB128 varints and the
//! MSB-first Huffman stream. These helpers are the single implementation of each encoding, so
//! the generator and the decoder cannot disagree about a boundary case.

/// Reads a LEB128 `u32` from the start of `bytes`, returning the value and its byte length.
///
/// Returns `None` for truncated or overlong input; a value wider than `u32` is rejected rather
/// than truncated.
#[must_use]
pub fn read_varint(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for (used, &byte) in bytes.iter().enumerate() {
        value |= u64::from(byte & 0x7F) << shift;
        if byte < 0x80 {
            return Some((u32::try_from(value).ok()?, used + 1));
        }
        shift += 7;
        if shift >= 32 {
            return None;
        }
    }
    None
}

/// Appends `value` as a LEB128 varint.
pub fn write_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "the low seven bits are kept and the continuation bit is added"
        )]
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "the loop leaves a value below 0x80"
    )]
    out.push(value as u8);
}

/// Writes `cp` as upper-case hex digits into `out`, returning how many bytes it wrote.
///
/// Shared by the runtime and the trainer so a `*` pattern name materializes identically on
/// both sides: the runtime fills the placeholder in `prefix + hex + suffix` when it composes
/// the name, and the trainer verifies every pattern range with the same format. `width` is a
/// *minimum*, the way Unicode writes the placeholder: `EGYPTIAN HIEROGLYPH-13460` needs five
/// digits where most code points need four. The digit count is clamped to the eight bytes of
/// `out`, so a wider `width` pads with leading zeros up to that bound instead of overflowing.
#[must_use]
pub fn write_hex(out: &mut [u8; 8], cp: u32, width: u8) -> usize {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut needed = 0usize;
    let mut rest = cp;
    while rest > 0 {
        rest >>= 4;
        needed += 1;
    }
    // Unicode always writes at least one digit, even for zero.
    let count = needed.max(usize::from(width.min(8))).max(1);
    let mut shift = (count - 1) * 4;
    for slot in out.iter_mut().take(count) {
        *slot = DIGITS[(cp >> shift) as usize & 0xF];
        shift = shift.saturating_sub(4);
    }
    count
}

/// Appends the name of a `*` pattern: `prefix + upper-case hex(cp) + suffix`.
///
/// The hex placeholder is padded to at least four digits, the way Unicode spells it; wider code
/// points grow beyond that (`U+18CFF`). Both sides build pattern names through this function so
/// the runtime and the trainer cannot disagree on the spelling.
pub fn write_pattern_name(prefix: &str, suffix: &str, cp: u32, out: &mut String) {
    let mut digits = [0u8; 8];
    let used = write_hex(&mut digits, cp, 4);
    out.push_str(prefix);
    for &digit in &digits[..used] {
        out.push(char::from(digit));
    }
    out.push_str(suffix);
}

/// Encodes `cps` as UTF-8 into `out`, returning how many bytes it wrote.
///
/// Returns `None` when a code point is not a scalar value or the encoding would overflow `out`.
#[must_use]
pub fn encode_cps(cps: &[u32], out: &mut [u8]) -> Option<usize> {
    let mut used = 0usize;
    for &cp in cps {
        let c = char::from_u32(cp)?;
        let end = used.checked_add(c.len_utf8())?;
        c.encode_utf8(out.get_mut(used..end)?);
        used = end;
    }
    Some(used)
}

/// Packs `values` into `width`-bit little-endian fields, low bits first.
///
/// `width` is the number of bits per field and must be in `1..=8`.
///
/// # Panics
///
/// Panics when `width` is outside `1..=8` or when a value does not fit its field. Both are
/// caller bugs: a silently truncated field would corrupt every following value.
#[must_use]
pub fn pack_bits(values: impl IntoIterator<Item = u8>, width: usize) -> Vec<u8> {
    assert!(
        (1..=8).contains(&width),
        "a packed field is one to eight bits wide, got {width}"
    );
    let values: Vec<u8> = values.into_iter().collect();
    let mut out = vec![0u8; (values.len() * width).div_ceil(8)];
    for (index, &value) in values.iter().enumerate() {
        assert!(
            usize::from(value) < 1 << width,
            "value {value} does not fit in {width} bits"
        );
        let bit = index * width;
        let shifted = u16::from(value) << (bit % 8);
        let [low, high] = shifted.to_le_bytes();
        out[bit / 8] |= low;
        if let Some(slot) = out.get_mut(bit / 8 + 1) {
            *slot |= high;
        }
    }
    out
}

/// Reads the `width`-bit field at `index` from a blob written by [`pack_bits`].
///
/// Returns `None` when `width` is outside `1..=8` or `index` is past the blob.
#[must_use]
pub fn unpack_bits(blob: &[u8], index: usize, width: usize) -> Option<u8> {
    if !(1..=8).contains(&width) {
        return None;
    }
    let bit = index.checked_mul(width)?;
    if bit.checked_add(width)? > blob.len().checked_mul(8)? {
        return None;
    }
    let byte = *blob.get(bit / 8)?;
    let next = blob.get(bit / 8 + 1).copied().unwrap_or(0);
    let value = (u16::from(byte) | (u16::from(next) << 8)) >> (bit % 8);
    u8::try_from(value & ((1u16 << width) - 1)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip_and_reject_overlong_input() {
        for value in [0u64, 1, 127, 128, 300, u64::from(u32::MAX)] {
            let mut encoded = Vec::new();
            write_varint(value, &mut encoded);
            assert_eq!(
                read_varint(&encoded),
                Some((
                    u32::try_from(value).expect("test values fit in u32"),
                    encoded.len()
                ))
            );
        }
        assert_eq!(read_varint(&[]), None);
        assert_eq!(read_varint(&[0x80]), None);
        assert_eq!(
            read_varint(&[0xFF, 0xFF, 0xFF, 0xFF, 0x10]),
            None,
            "a payload wider than u32 must not truncate"
        );
    }

    #[test]
    fn write_hex_pads_and_widens_as_unicode_spells_it() {
        let mut digits = [0u8; 8];
        assert_eq!(write_hex(&mut digits, 0, 0), 1);
        assert_eq!(&digits[..1], b"0");
        assert_eq!(write_hex(&mut digits, 0, 4), 4);
        assert_eq!(&digits[..4], b"0000");
        assert_eq!(write_hex(&mut digits, 0x4E2D, 0), 4);
        assert_eq!(&digits[..4], b"4E2D");
        // `EGYPTIAN HIEROGLYPH-13460` needs five digits where the width asks for four.
        assert_eq!(write_hex(&mut digits, 0x13460, 4), 5);
        assert_eq!(&digits[..5], b"13460");
        // A width wider than the buffer is clamped rather than panicking.
        assert_eq!(write_hex(&mut digits, 1, u8::MAX), 8);
        assert_eq!(&digits[..8], b"00000001");
    }

    #[test]
    fn pattern_names_share_one_placeholder_format() {
        let mut name = String::from("x");
        write_pattern_name("CJK UNIFIED IDEOGRAPH-", "", 0x3400, &mut name);
        assert_eq!(name, "xCJK UNIFIED IDEOGRAPH-3400");
        name.clear();
        write_pattern_name("KHITAN SMALL SCRIPT CHARACTER-", "-X", 0x18CFF, &mut name);
        assert_eq!(name, "KHITAN SMALL SCRIPT CHARACTER-18CFF-X");
    }

    #[test]
    fn encode_cps_writes_utf8_and_rejects_invalid_scalars() {
        let mut buffer = [0u8; 16];
        let used = encode_cps(&[0x64, 0x1F994], &mut buffer).expect("scalars fit");
        assert_eq!(&buffer[..used], "d🦔".as_bytes());
        assert_eq!(encode_cps(&[0xD800], &mut buffer), None);
        let too_long = [0x1F994u32; 17];
        assert_eq!(encode_cps(&too_long, &mut buffer), None);
    }

    #[test]
    fn packed_fields_round_trip_at_every_width() {
        let values = [0u8, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144, 233, 255];
        for width in 1..=8 {
            let fits: Vec<u8> = values
                .iter()
                .map(|&value| value & u8::try_from((1u16 << width) - 1).expect("a byte mask"))
                .collect();
            let blob = pack_bits(fits.iter().copied(), width);
            for (index, &value) in fits.iter().enumerate() {
                assert_eq!(
                    unpack_bits(&blob, index, width),
                    Some(value),
                    "width {width}, index {index}"
                );
            }
            // The final byte is padded, so only an index whose field starts past the blob is
            // rejected; a field inside the padding reads as zero.
            assert_eq!(unpack_bits(&blob, fits.len() + 8, width), None);
        }
        assert_eq!(pack_bits(std::iter::empty(), 3), Vec::<u8>::new());
        assert_eq!(unpack_bits(&[], 0, 3), None);
        assert_eq!(unpack_bits(&[0xFF], 0, 0), None);
        assert_eq!(unpack_bits(&[0xFF], 0, 9), None);
    }

    #[test]
    #[should_panic(expected = "does not fit in 3 bits")]
    fn a_value_wider_than_its_field_is_rejected() {
        let _ = pack_bits([8u8], 3);
    }
}
