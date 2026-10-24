//! Nomina `*`-pattern lookup: varint ranges plus prefix and suffix dictionaries.
//!
//! The section stores the twenty `DerivedName.txt` ranges that spell a name as
//! `prefix + hex(cp) + suffix`. Prefixes and suffixes are dictionary strings, so the eight
//! distinct prefixes cost their bytes once; each range is a start delta, a length and two
//! dictionary ids.
//!
//! # Layout
//!
//! ```text
//! u8 pattern_count | u8 prefix_count | prefix_count * (u8 len, bytes)
//! u8 suffix_count | suffix_count * (u8 len, bytes)
//! pattern_count * (varint start_delta, varint length, u8 prefix_id, u8 suffix_id)
//! ```

use std::borrow::Cow;

use zeta_unicode_format::write_hex;

/// The most dictionary strings the fixed lookup arrays hold.
const MAX_DICTIONARY: usize = 64;
/// The most pattern ranges a section may hold.
const MAX_PATTERNS: usize = 256;

/// Returns the name of `cp` when a `*` pattern covers it.
///
/// The name is `prefix + hex(cp) + suffix`, with the code point written the way Unicode writes
/// its placeholder: upper case, no padding, at least one digit (`U+18CFF` is five digits).
/// Returns `None` for a truncated section and for a code point outside every range.
#[must_use]
pub fn pattern_name(section: &[u8], cp: u32) -> Option<Cow<'static, str>> {
    let mut at = 0usize;
    let pattern_count = usize::from(*section.get(at)?);
    at += 1;
    if pattern_count > MAX_PATTERNS {
        return None;
    }

    let mut prefixes = [(0usize, 0usize); MAX_DICTIONARY];
    let mut prefix_count = 0usize;
    let count = usize::from(*section.get(at)?);
    at += 1;
    if count > MAX_DICTIONARY {
        return None;
    }
    for slot in prefixes.iter_mut().take(count) {
        let length = usize::from(*section.get(at)?);
        at += 1;
        section.get(at..at.checked_add(length)?)?;
        at += length;
        *slot = (at - length, length);
        prefix_count += 1;
    }

    let mut suffixes = [(0usize, 0usize); MAX_DICTIONARY];
    let mut suffix_count = 0usize;
    let count = usize::from(*section.get(at)?);
    at += 1;
    if count > MAX_DICTIONARY {
        return None;
    }
    for slot in suffixes.iter_mut().take(count) {
        let length = usize::from(*section.get(at)?);
        at += 1;
        section.get(at..at.checked_add(length)?)?;
        at += length;
        *slot = (at - length, length);
        suffix_count += 1;
    }

    let mut start = 0u32;
    for _ in 0..pattern_count {
        let (delta, used) = zeta_unicode_format::read_varint(section.get(at..)?)?;
        at += used;
        start = start.checked_add(delta)?;
        let (length, used) = zeta_unicode_format::read_varint(section.get(at..)?)?;
        at += used;
        let prefix_id = usize::from(*section.get(at)?);
        at += 1;
        let suffix_id = usize::from(*section.get(at)?);
        at += 1;
        if prefix_id >= prefix_count || suffix_id >= suffix_count {
            return None;
        }
        if cp < start {
            return None;
        }
        let end = start.checked_add(length)?;
        if cp >= end {
            continue;
        }
        let (prefix_at, prefix_len) = prefixes[prefix_id];
        let (suffix_at, suffix_len) = suffixes[suffix_id];
        let prefix = std::str::from_utf8(section.get(prefix_at..prefix_at + prefix_len)?).ok()?;
        let suffix = std::str::from_utf8(section.get(suffix_at..suffix_at + suffix_len)?).ok()?;
        let mut out = String::with_capacity(prefix.len() + 8 + suffix.len());
        out.push_str(prefix);
        let mut digits = [0u8; 8];
        let used = write_hex(&mut digits, cp, 0);
        out.push_str(std::str::from_utf8(digits.get(..used)?).ok()?);
        out.push_str(suffix);
        return Some(Cow::Owned(out));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a section with an explicit prefix and suffix dictionary.
    fn section(prefixes: &[&str], suffixes: &[&str], patterns: &[(u32, u32, u8, u8)]) -> Vec<u8> {
        let mut out = vec![u8::try_from(patterns.len()).unwrap()];
        for dictionary in [prefixes, suffixes] {
            out.push(u8::try_from(dictionary.len()).unwrap());
            for text in dictionary {
                out.push(u8::try_from(text.len()).unwrap());
                out.extend_from_slice(text.as_bytes());
            }
        }
        let mut previous = 0u32;
        for &(lo, hi, prefix, suffix) in patterns {
            zeta_unicode_format::write_varint(u64::from(lo - previous), &mut out);
            previous = lo;
            zeta_unicode_format::write_varint(u64::from(hi - lo + 1), &mut out);
            out.push(prefix);
            out.push(suffix);
        }
        out
    }

    #[test]
    fn patterns_write_upper_case_hex_without_padding() {
        let blob = section(
            &["TEST-", "LONGER-"],
            &["", "-END"],
            &[(0x10, 0x12, 0, 0), (0x18CFF, 0x18CFF, 1, 1)],
        );
        assert_eq!(pattern_name(&blob, 0x10).as_deref(), Some("TEST-10"));
        assert_eq!(pattern_name(&blob, 0x11).as_deref(), Some("TEST-11"));
        assert_eq!(
            pattern_name(&blob, 0x18CFF).as_deref(),
            Some("LONGER-18CFF-END")
        );
        assert_eq!(pattern_name(&blob, 0xF), None);
        assert_eq!(pattern_name(&blob, 0x13), None);
        assert_eq!(pattern_name(&blob, 0x18D00), None);
        assert_eq!(pattern_name(&[], 0), None);
    }

    #[test]
    fn truncated_sections_are_rejected() {
        let blob = section(&["A-"], &[""], &[(0x41, 0x41, 0, 0)]);
        assert_eq!(pattern_name(&blob, 0x41).as_deref(), Some("A-41"));
        for length in 0..blob.len() {
            assert_eq!(pattern_name(&blob[..length], 0x41), None, "length {length}");
        }
    }
}
