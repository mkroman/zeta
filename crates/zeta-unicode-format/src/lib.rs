//! Layout constants and emoji tone formatting shared by `zeta-unicode` and `zeta-unicode-build`.
//!
//! The runtime crate decodes the table the offline generator writes, and both sides have to
//! agree on every byte of the format: the container headers, the section ids and the page size
//! of the general-category table. The generator also has to reproduce the runtime's skin-tone
//! wording exactly, because it selects the rule that the runtime later replays.
//!
//! Keeping those definitions here means a format change cannot update only one side. This crate
//! is deliberately dependency-free: it is linked into the bot through `zeta-unicode` and into
//! the build script through `zeta-unicode-build`.

/// The five emoji skin-tone code points, in tone order.
pub const TONES: [u32; 5] = [0x1F3FB, 0x1F3FC, 0x1F3FD, 0x1F3FE, 0x1F3FF];

/// The CLDR names of the five skin tones, in the same order as [`TONES`].
pub const TONE_NAMES: [&str; 5] = [
    "light skin tone",
    "medium-light skin tone",
    "medium skin tone",
    "medium-dark skin tone",
    "dark skin tone",
];

/// Longest canonical emoji key, in bytes.
///
/// The runtime keeps a fixed scratch buffer of this size; the generator rejects a stored key
/// that would not fit.
pub const MAX_KEY: usize = 64;

/// The first Hangul syllable, `HANGUL SYLLABLE GA`.
pub const HANGUL_START: u32 = 0xAC00;
/// The last Hangul syllable.
pub const HANGUL_END: u32 = 0xD7A3;
/// The number of Hangul syllables in Unicode 18.0.0.
pub const HANGUL_COUNT: usize = 11_172;

/// The first Unicode scalar value.
pub const UNICODE_START: u32 = 0;
/// The last Unicode scalar value.
pub const UNICODE_END: u32 = 0x10_FFFF;
/// The number of code points in the Unicode range, including surrogates.
pub const UNICODE_LIMIT: u32 = 0x11_0000;

/// `cp >> GC_SHIFT` selects a page of the general-category table.
pub const GC_SHIFT: u32 = 11;
/// The number of pages in the general-category table.
///
/// This is one more than [`UNICODE_END`] needs, so the directory always has a page after the
/// last scalar value and `ucd[page + 1]` is available for the run search.
pub const GC_PAGES: usize = (UNICODE_LIMIT as usize >> GC_SHIFT) + 1;

/// The general category abbreviations in table id order (id = position + 1).
///
/// The generator writes these ids and the runtime indexes this array, so both must use the same
/// order.
pub const GC_SHORT_NAMES: [&str; 30] = [
    "Cc", "Cf", "Cn", "Co", "Cs", "Ll", "Lm", "Lo", "Lt", "Lu", "Mc", "Me", "Mn", "Nd", "Nl", "No",
    "Pc", "Pd", "Pe", "Pf", "Pi", "Po", "Ps", "Sc", "Sk", "Sm", "So", "Zl", "Zp", "Zs",
];

/// The magic at the start of the generated table.
pub const TABLE_MAGIC: &[u8; 4] = b"ZUCD";
/// Bytes before the table's section directory: magic, three version components and a count.
pub const TABLE_HEADER_LEN: usize = 4 + 2 * 3 + 2;
/// Length of one table section directory entry: id, offset and payload length.
pub const TABLE_ENTRY_LEN: usize = 12;

/// The name index section.
pub const SECTION_NAME_RUNS: u32 = 1;
/// The `*`-pattern ranges section.
pub const SECTION_NAME_PATTERNS: u32 = 2;
/// The general-category section.
pub const SECTION_GC: u32 = 3;
/// The emoji section.
pub const SECTION_EMOJI: u32 = 4;
/// The name streams section.
pub const SECTION_NAMES: u32 = 5;

/// The magic at the start of the name-stream container.
pub const NAMES_MAGIC: &[u8; 4] = b"UNNM";
/// The name-container version understood by the runtime and the generator.
pub const NAMES_VERSION: u8 = 1;
/// Codec id: PPMd8-I through `ppmd-rust`.
pub const CODEC_PPMD8I: u8 = 1;
/// Stream transform id: no transform.
pub const TRANSFORM_RAW: u8 = 0;
/// Stream transform id: cumulative (delta) sum of `u8`s.
pub const TRANSFORM_DELTA8: u8 = 1;
/// The number of compressed name streams.
pub const STREAM_COUNT: usize = 3;
/// Length of one stream descriptor in the name-container header.
pub const STREAM_DESC_LEN: usize = 11;
/// Length of the fixed name-container header.
pub const NAMES_HEADER_LEN: usize = 4 + 1 + 1 + 3 + 4 + 4 + STREAM_COUNT * STREAM_DESC_LEN;
/// `u32` raw length, `u8` model order and `u8` memory limit on a standalone section.
pub const SECTION_PREFIX_LEN: usize = 6;

/// Is `cp` one of the five emoji skin-tone components?
#[must_use]
pub const fn is_tone(cp: u32) -> bool {
    let mut index = 0;
    while index < TONES.len() {
        if TONES[index] == cp {
            return true;
        }
        index += 1;
    }
    false
}

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
        #[allow(clippy::cast_possible_truncation)]
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    #[allow(clippy::cast_possible_truncation)]
    out.push(value as u8);
}

/// Applies cumulative (delta-8) coding to `values`.
#[must_use]
pub fn delta8(values: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len());
    let mut previous = 0u8;
    for &value in values {
        out.push(value.wrapping_sub(previous));
        previous = value;
    }
    out
}

/// Turns delta-8 coding back into the values it coded.
pub fn undelta8(values: &mut [u8]) {
    let mut previous = 0u8;
    for value in values.iter_mut() {
        *value = value.wrapping_add(previous);
        previous = *value;
    }
}

/// The indices into [`TONES`] of the skin-tone code points in `cps`, in order.
pub fn tone_positions(cps: impl IntoIterator<Item = u32>) -> Vec<usize> {
    cps.into_iter()
        .filter_map(|cp| TONES.iter().position(|&tone| tone == cp))
        .collect()
}

/// Renders `base` with `tones` applied, following the 3-bit rule.
///
/// Rule `1` inserts the tone phrase after the first `": "`, rule `2` appends it, and rules
/// `3`/`4` do the same after collapsing consecutive duplicate tones.
#[must_use]
pub fn format_toned(code: u8, base: &str, tones: &[usize]) -> String {
    let phrase = tone_phrase(code, tones);

    match (base.find(": "), code) {
        (Some(colon), 1 | 3) => {
            format!("{}: {}, {}", &base[..colon], phrase, &base[colon + 2..])
        }
        (Some(_), _) => format!("{base}, {phrase}"),
        (None, _) => format!("{base}: {phrase}"),
    }
}

/// The comma-joined tone names, collapsing consecutive duplicates for rules 3 and 4.
#[must_use]
pub fn tone_phrase(code: u8, tones: &[usize]) -> String {
    let dedup = matches!(code, 3 | 4);
    let mut phrase = String::new();
    let mut previous = None;
    for &index in tones {
        if dedup && previous == Some(index) {
            continue;
        }
        if !phrase.is_empty() {
            phrase.push_str(", ");
        }
        phrase.push_str(TONE_NAMES[index]);
        previous = Some(index);
    }
    phrase
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_positions_finds_every_tone_in_order() {
        assert_eq!(tone_positions([0x1F3FF, 0x41, 0x1F3FB]), [4, 0]);
        assert_eq!(tone_positions(std::iter::empty()), []);
    }

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
    fn delta8_round_trips() {
        let values = [0u8, 5, 5, 200, 1];
        let mut coded = delta8(&values);
        assert_ne!(coded, values);
        undelta8(&mut coded);
        assert_eq!(coded, values);
    }

    #[test]
    fn is_tone_accepts_only_the_five_components() {
        for tone in TONES {
            assert!(is_tone(tone));
        }
        assert!(!is_tone(0x1F3FA));
        assert!(!is_tone(0x1F400));
    }

    #[test]
    fn formats_a_toned_name_with_every_rule() {
        // Rule 1 inserts after the first `": "`; rule 3 does the same and deduplicates.
        assert_eq!(
            format_toned(1, "handshake: base", &[0]),
            "handshake: light skin tone, base"
        );
        assert_eq!(
            format_toned(3, "women holding hands: base", &[0, 0]),
            "women holding hands: light skin tone, base"
        );
        // Rules 2 and 4 append the phrase.
        assert_eq!(
            format_toned(2, "thumbs up", &[0, 1]),
            "thumbs up: light skin tone, medium-light skin tone"
        );
        assert_eq!(
            format_toned(4, "thumbs up: base", &[2, 2]),
            "thumbs up: base, medium skin tone"
        );
        // A base without a `": "` always appends after a colon.
        assert_eq!(
            format_toned(1, "thumbs up", &[4]),
            "thumbs up: dark skin tone"
        );
    }

    #[test]
    fn a_tone_phrase_keeps_non_consecutive_duplicates() {
        assert_eq!(
            tone_phrase(3, &[0, 1, 0]),
            "light skin tone, medium-light skin tone, light skin tone"
        );
        assert_eq!(tone_phrase(2, &[0, 0]), "light skin tone, light skin tone");
    }

    #[test]
    fn the_layout_constants_are_self_consistent() {
        assert_eq!(NAMES_HEADER_LEN, 50);
        assert_eq!(GC_PAGES, 545);
        assert_eq!(GC_SHORT_NAMES.len(), 30);
        assert_eq!(UNICODE_START, 0);
        assert_eq!(UNICODE_LIMIT, UNICODE_END + 1);
        assert_eq!(UNICODE_LIMIT as usize >> GC_SHIFT, GC_PAGES - 1);
        assert_eq!(
            HANGUL_END - HANGUL_START + 1,
            u32::try_from(HANGUL_COUNT).expect("the count fits in u32")
        );
    }
}
