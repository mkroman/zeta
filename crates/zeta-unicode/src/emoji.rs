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
    names: &'a str,
    name_offsets: &'a [u8],
    codes: &'a [u8],
    tone_masks: &'a [u8],
    /// One bit per possible first key byte, for the cheap admission check.
    first_bytes: [u64; 4],
    entries: usize,
}

/// The parsed emoji section, initialized on the first emoji lookup.
static EMOJI: OnceLock<Option<Emoji<'static>>> = OnceLock::new();

/// Parses the emoji section now, so the first lookup does not pay for it.
///
/// Idempotent and cheap once the section exists.
pub fn init() {
    let _ = Emoji::section();
}

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
        let keys = tables::bytes_at(section, keys_at, key_bytes)?;
        let key_offsets = tables::bytes_at(section, key_offsets_at, offsets)?;
        let first_bytes = Self::first_bytes(keys, key_offsets, entries)?;
        Some(Self {
            keys,
            key_offsets,
            names: std::str::from_utf8(tables::bytes_at(section, names_at, name_bytes)?).ok()?,
            name_offsets: tables::bytes_at(section, name_offsets_at, offsets)?,
            codes: tables::bytes_at(section, codes_at, code_bytes)?,
            tone_masks: tables::bytes_at(section, tone_masks_at, mask_bytes)?,
            first_bytes,
            entries,
        })
    }

    /// Builds the first-byte admission bitmap from every stored key.
    fn first_bytes(keys: &[u8], key_offsets: &[u8], entries: usize) -> Option<[u64; 4]> {
        let mut first_bytes = [0u64; 4];
        for index in 0..entries {
            let (start, end) = tables::u16_pair_at(key_offsets, index)?;
            let start = usize::from(start);
            let key = tables::bytes_at(keys, start, usize::from(end).checked_sub(start)?)?;
            if let Some(&byte) = key.first() {
                first_bytes[usize::from(byte) >> 6] |= 1 << (byte & 63);
            }
        }
        Some(first_bytes)
    }

    /// Returns whether a stored key can start with `byte`.
    fn admits(&self, byte: u8) -> bool {
        self.first_bytes[usize::from(byte) >> 6] & (1 << (byte & 63)) != 0
    }

    /// Returns the parsed section, or `None` when the table has no emoji section.
    fn section() -> Option<&'static Self> {
        EMOJI
            .get_or_init(|| tables::section(sections::EMOJI).and_then(Self::parse))
            .as_ref()
    }

    /// Returns entry `index` out of `blob`, using the matching `u16` offset table.
    fn blob(blob: &'a [u8], offsets: &[u8], index: usize) -> Option<&'a [u8]> {
        let (start, end) = tables::u16_pair_at(offsets, index)?;
        let start = usize::from(start);
        tables::bytes_at(blob, start, usize::from(end).checked_sub(start)?)
    }

    /// Returns key `index` out of the key blob.
    fn key(&self, index: usize) -> Option<&'a [u8]> {
        Self::blob(self.keys, self.key_offsets, index)
    }

    /// Returns name `index` out of the validated name blob.
    fn name(&self, index: usize) -> Option<&'a str> {
        let (start, end) = tables::u16_pair_at(self.name_offsets, index)?;
        self.names.get(usize::from(start)..usize::from(end))
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
        let base_name = self.name(index)?;
        Some(Cow::Owned(format_toned(
            code,
            base_name,
            &tones[..tone_count],
        )))
    }
}

/// Returns whether `sequence` could start a stored emoji sequence.
///
/// This is an admission check, not a match: the first byte is tested against a bitmap built
/// from every stored key, so `true` means the sequence is worth a lookup, never that it is an
/// emoji. Empty input answers `false`.
#[must_use]
pub fn may_start_emoji_sequence(sequence: &str) -> bool {
    let Some(&first) = sequence.as_bytes().first() else {
        return false;
    };
    Emoji::section().is_some_and(|emoji| emoji.admits(first))
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
    let leading = *sequence.as_bytes().first()?;
    let mut chars = sequence.chars();
    let first = chars.next()?;
    // A lone skin-tone component is an emoji entry; a tone or variation selector in front of
    // anything else is not a sequence.
    if chars.next().is_some() && (is_tone(u32::from(first)) || first == '\u{FE0F}') {
        return None;
    }

    // Reject impossible sequences before touching the MAX_KEY buffers.
    let emoji = Emoji::section()?;
    if !emoji.admits(leading) {
        return None;
    }

    let mut buffer = [0u8; MAX_KEY];
    let needle = canonical_key(sequence.as_bytes(), &mut buffer)?;
    if let Some(index) = emoji.find(needle) {
        return Some(Cow::Borrowed(emoji.name(index)?));
    }

    // Only the synthesis fallback needs the second buffer.
    let mut scratch = [0u8; MAX_KEY];
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
    fn admission_rejects_impossible_first_bytes() {
        assert!(!may_start_emoji_sequence(""));
        assert!(!may_start_emoji_sequence("d'être"));
        assert!(may_start_emoji_sequence("🏳️‍🌈"));
        // A keycap sequence stored by `NamedSequences.txt` starts with the digit.
        assert!(may_start_emoji_sequence("0"));
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
