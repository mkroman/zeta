//! Emoji sequence lookup: canonical keys, binary search and skin-tone synthesis.
//!
//! Keys are canonicalized by stripping `U+FE0F`, and skin-tone variants are not stored: the
//! table records a 3-bit formatting rule and a 2-bit published-tone-count mask per base, so a
//! lookup strips the tones, finds the base and renders the name.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::sync::OnceLock;

use zeta_unicode_format::{MAX_KEY, format_toned, is_tone, tone_indices};

use crate::sections;
use crate::tables;

/// The emoji section, split into the blobs and offset tables it stores.
struct Emoji<'a> {
    keys: &'a [u8],
    key_offsets: &'a [u8],
    names: &'a [u8],
    name_offsets: &'a [u8],
    codes: &'a [u8],
    tone_masks: &'a [u8],
    entries: usize,
}

/// The parsed emoji section, initialized on the first emoji lookup.
static EMOJI: OnceLock<Option<Emoji<'static>>> = OnceLock::new();

impl<'a> Emoji<'a> {
    /// Splits a section payload into its parts: entry count, key blob with `u16` offsets, name
    /// blob with `u16` offsets, the packed 3-bit tone rules and the 2-bit tone-count masks.
    fn parse(section: &'a [u8]) -> Option<Self> {
        let entries = usize::from(tables::u16_at(section, 0)?);
        let key_bytes = usize::from(tables::u16_at(section, 2)?);
        let name_bytes = usize::from(tables::u16_at(section, 4)?);
        let code_bytes = usize::from(tables::u16_at(section, 6)?);
        let mask_bytes = usize::from(tables::u16_at(section, 8)?);
        let offsets = entries.checked_add(1)?.checked_mul(2)?;
        let keys_at: usize = 10;
        let key_offsets_at = keys_at.checked_add(key_bytes)?;
        let names_at = key_offsets_at.checked_add(offsets)?;
        let name_offsets_at = names_at.checked_add(name_bytes)?;
        let codes_at = name_offsets_at.checked_add(offsets)?;
        let tone_masks_at = codes_at.checked_add(code_bytes)?;
        Some(Self {
            keys: tables::bytes_at(section, keys_at, key_bytes)?,
            key_offsets: tables::bytes_at(section, key_offsets_at, offsets)?,
            names: tables::bytes_at(section, names_at, name_bytes)?,
            name_offsets: tables::bytes_at(section, name_offsets_at, offsets)?,
            codes: tables::bytes_at(section, codes_at, code_bytes)?,
            tone_masks: tables::bytes_at(section, tone_masks_at, mask_bytes)?,
            entries,
        })
    }

    /// Returns the parsed section, or `None` when the table has no emoji section.
    fn section() -> Option<&'static Self> {
        EMOJI
            .get_or_init(|| tables::section(sections::EMOJI).and_then(Self::parse))
            .as_ref()
    }

    /// Returns entry `index` out of `blob`, using the matching `u16` offset table.
    fn blob(blob: &'a [u8], offsets: &[u8], index: usize) -> Option<&'a [u8]> {
        let start = usize::from(tables::u16_at(offsets, index.checked_mul(2)?)?);
        let end = usize::from(tables::u16_at(
            offsets,
            index.checked_mul(2)?.checked_add(2)?,
        )?);
        tables::bytes_at(blob, start, end.checked_sub(start)?)
    }

    /// Returns key `index` out of the key blob.
    fn key(&self, index: usize) -> Option<&'a [u8]> {
        Self::blob(self.keys, self.key_offsets, index)
    }

    /// Returns name `index` out of the name blob.
    fn name(&self, index: usize) -> Option<&'a [u8]> {
        Self::blob(self.names, self.name_offsets, index)
    }

    /// Reads a packed `width`-bit field of `blob`, which may straddle a byte boundary.
    fn packed(blob: &[u8], index: usize, width: usize) -> Option<u8> {
        let bit = index.checked_mul(width)?;
        let byte = blob.get(bit / 8)?;
        let next = blob.get(bit / 8 + 1).copied().unwrap_or(0);
        let value = (u16::from(*byte) | (u16::from(next) << 8)) >> (bit % 8);
        Some(u8::try_from(value & ((1u16 << width) - 1)).expect("the field fits in a byte"))
    }

    /// Returns the 3-bit tone formatting rule of entry `index`.
    fn tone_code(&self, index: usize) -> Option<u8> {
        Self::packed(self.codes, index, 3)
    }

    /// Returns the 2-bit published-tone-count mask of entry `index`: bit 0 allows one tone,
    /// bit 1 allows two.
    fn tone_mask(&self, index: usize) -> Option<u8> {
        Self::packed(self.tone_masks, index, 2)
    }

    /// Binary-searches the stored keys for `needle`.
    fn find(&self, needle: &[u8]) -> Option<usize> {
        let (mut low, mut high) = (0usize, self.entries);
        while low < high {
            let mid = low + (high - low) / 2;
            match needle.cmp(self.key(mid)?) {
                Ordering::Less => high = mid,
                Ordering::Greater => low = mid + 1,
                Ordering::Equal => return Some(mid),
            }
        }
        None
    }

    /// Resolves `needle` (already canonicalized) by stripping its skin tones and synthesizing
    /// the base name with that entry's formatting rule.
    fn tone_variant(
        &self,
        needle: &[u8],
        scratch: &mut [u8; MAX_KEY],
    ) -> Option<Cow<'static, str>> {
        let text = std::str::from_utf8(needle).ok()?;
        let (tones, tone_count) = tone_indices(text.chars().map(u32::from))?;
        if !(1..=2).contains(&tone_count) {
            return None;
        }

        let base = strip_tones(text, scratch)?;
        let index = self.find(base)?;
        let code = self.tone_code(index)?;
        if code == 0 {
            return None;
        }
        let mask = self.tone_mask(index)?;
        if mask & (1 << (tone_count - 1)) == 0 {
            return None;
        }
        let base_name = std::str::from_utf8(self.name(index)?).ok()?;
        Some(Cow::Owned(format_toned(
            code,
            base_name,
            &tones[..tone_count],
        )))
    }
}

/// Returns the CLDR short name of an emoji sequence such as `🏳️‍🌈`.
///
/// Names come from `emoji-test.txt`, or from `NamedSequences.txt` for the official name of an
/// emoji sequence such as a keycap. Non-emoji named sequences such as Latin letters with
/// combining marks are not part of this table.
///
/// Keys are canonicalized: `U+FE0F` is optional (`🚶‍♂` answers like `🚶‍♂️`) and skin-tone
/// variants resolve by synthesizing the base name with the recorded formatting rule. The
/// published tone count is checked, and a leading tone or `U+FE0F` is rejected, but the
/// positions of the tones are not: an unlisted arrangement with a published tone count can
/// still resolve. The name is returned as published — lower case for emoji, except the keycap
/// names from `NamedSequences.txt`, which are upper case — so callers can case it however their
/// reply needs. Returns `None` for non-emoji text and for sequences whose base or tone count is
/// not known.
#[must_use]
pub fn emoji_name(sequence: &str) -> Option<Cow<'static, str>> {
    let mut chars = sequence.chars();
    let first = chars.next()?;
    // A lone skin-tone component is an emoji entry; a tone or variation selector in front of
    // anything else is not a sequence.
    if chars.next().is_some() && (is_tone(u32::from(first)) || first == '\u{FE0F}') {
        return None;
    }

    let mut buffer = [0u8; MAX_KEY];
    let mut scratch = [0u8; MAX_KEY];
    let needle = canonical_key(sequence.as_bytes(), &mut buffer)?;
    let emoji = Emoji::section()?;

    if let Some(index) = emoji.find(needle) {
        let name = emoji.name(index)?;
        return Some(Cow::Borrowed(std::str::from_utf8(name).ok()?));
    }
    emoji.tone_variant(needle, &mut scratch)
}

/// Strips `U+FE0F` (`EF B8 8F`) into `buffer` without allocating.
///
/// Returns `None` when the canonical form does not fit; no stored key is longer than
/// [`MAX_KEY`], so an overlong input is a certain miss.
fn canonical_key<'a>(key: &[u8], buffer: &'a mut [u8; MAX_KEY]) -> Option<&'a [u8]> {
    let mut used = 0usize;
    let mut index = 0usize;
    while index < key.len() {
        if key[index] == 0xEF
            && key.get(index + 1) == Some(&0xB8)
            && key.get(index + 2) == Some(&0x8F)
        {
            index += 3;
            continue;
        }
        *buffer.get_mut(used)? = *key.get(index)?;
        used += 1;
        index += 1;
    }
    buffer.get(..used)
}

/// Copies `text` into `buffer` without its skin-tone characters, so the tone fallback needs no
/// allocation.
fn strip_tones<'a>(text: &str, buffer: &'a mut [u8; MAX_KEY]) -> Option<&'a [u8]> {
    let mut used = 0usize;
    for c in text.chars() {
        if is_tone(u32::from(c)) {
            continue;
        }
        let end = used.checked_add(c.len_utf8())?;
        let mut encoded = [0u8; 4];
        buffer
            .get_mut(used..end)?
            .copy_from_slice(c.encode_utf8(&mut encoded).as_bytes());
        used = end;
    }
    buffer.get(..used)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_key_strips_every_variation_selector() {
        let mut buffer = [0u8; MAX_KEY];
        let key = canonical_key(b"a\xEF\xB8\x8Fb\xEF\xB8\x8F", &mut buffer);
        assert_eq!(key, Some(b"ab".as_slice()));
    }

    #[test]
    fn canonical_key_rejects_a_key_that_does_not_fit() {
        let mut buffer = [0u8; MAX_KEY];
        let exactly = vec![b'a'; MAX_KEY];
        assert_eq!(
            canonical_key(&exactly, &mut buffer),
            Some(exactly.as_slice())
        );
        let mut buffer = [0u8; MAX_KEY];
        assert_eq!(canonical_key(&[b'a'; MAX_KEY + 1], &mut buffer), None);
    }

    #[test]
    fn emoji_name_rejects_empty_and_selector_only_input() {
        assert_eq!(emoji_name(""), None);
        assert_eq!(emoji_name("\u{FE0F}"), None);
        assert_eq!(emoji_name("👍🏻🏿🏾"), None);
    }

    #[test]
    fn tone_fallback_synthesizes_known_names() {
        assert_eq!(
            emoji_name("🫶🏻").as_deref(),
            Some("heart hands: light skin tone")
        );
        assert_eq!(
            emoji_name("🫱🏻‍🫲🏿").as_deref(),
            Some("handshake: light skin tone, dark skin tone")
        );
    }

    #[test]
    fn a_missing_emoji_section_is_not_an_error() {
        let section = tables::section(sections::EMOJI).expect("the table has an emoji section");
        assert!(Emoji::parse(section).is_some());
        assert!(Emoji::parse(&section[..5]).is_none());
        assert!(Emoji::parse(&[]).is_none());
    }
}
