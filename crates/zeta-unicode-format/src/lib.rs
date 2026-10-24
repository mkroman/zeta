//! Layout constants and emoji grammar shared by `zeta-unicode` and `zeta-unicode-build`.
//!
//! The runtime crate and the offline trainer share the wire-format constants, the byte
//! primitives, the general-category encoding, the Hangul and emoji tone tables, the name-pattern
//! hex formatter and the emoji name grammar. The generator has to reproduce the runtime's
//! wording exactly, because it selects the rule that the runtime later replays; keeping one
//! implementation means the two can never drift.
//!
//! This crate is deliberately dependency-free: it is linked into the bot through
//! `zeta-unicode` and into the build script through `zeta-unicode-build`.

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

const _: () = assert!(TONES.len() == TONE_NAMES.len());

/// Longest canonical emoji key, in bytes.
///
/// The runtime keeps a fixed scratch buffer of this size; the generator rejects a stored key
/// that would not fit.
pub const MAX_KEY: usize = 64;

/// The most code points one stored emoji key may have.
pub const MAX_CPS: usize = 16;

/// The `U+200D` zero-width joiner.
pub const ZWJ: u32 = 0x200D;

/// The largest rule argument the packed `(argument << 4) | rule` entry encodes.
pub const MAX_RULE_ARGUMENT: u16 = 0x0FFF;

/// Rule id `literal`: the name is stored in the literal-name arena.
pub const RULE_LITERAL: u8 = 0;
/// Rule id `ucd_mask`: select words from the lowercase UCD name.
pub const RULE_UCD_MASK: u8 = 1;
/// Rule id `ucd_mask_hyphen`: select words, joining the first two with a hyphen.
pub const RULE_UCD_MASK_HYPHEN: u8 = 2;
/// Rule id `keycap`: `KEYCAP ` plus the keycap word.
pub const RULE_KEYCAP: u8 = 3;
/// Rule id `gender_w`: swap `person` for `woman` in a base name.
pub const RULE_GENDER_WOMAN: u8 = 4;
/// Rule id `gender_m`: swap `person` for `man` in a base name.
pub const RULE_GENDER_MAN: u8 = 5;
/// Rule id `facing`: base name plus ` facing right`.
pub const RULE_FACING: u8 = 6;
/// Rule id `family`: `family: ` plus comma-joined family words.
pub const RULE_FAMILY: u8 = 7;
/// Rule id `couple`: `couple with heart: ` plus comma-joined family words.
pub const RULE_COUPLE: u8 = 8;
/// Rule id `kiss`: `kiss: ` plus comma-joined family words.
pub const RULE_KISS: u8 = 9;
/// Rule id `pair`: plural word plus the pair phrase.
pub const RULE_PAIR: u8 = 10;
/// Rule id `tone_component`: one of the five skin-tone names.
pub const RULE_TONE_COMPONENT: u8 = 11;
/// Rule id `clock`: the hour word plus ` o’clock`.
pub const RULE_CLOCK: u8 = 12;
/// Rule id `keycap10`: the literal name `keycap: 10`.
pub const RULE_KEYCAP_10: u8 = 13;

/// The number of grammar rules, literal included.
pub const RULE_COUNT: usize = 14;

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
/// This is one more than [`UNICODE_END`] needs, so the runtime's page directory carries a
/// final sentinel entry and the run search on the last page has an upper bound.
pub const GC_PAGES: usize = (UNICODE_LIMIT as usize >> GC_SHIFT) + 1;
/// The low bits of a general-category record holding the category id.
pub const GC_ID_BITS: u32 = 5;
/// The mask that extracts the category id from a general-category record.
pub const GC_ID_MASK: u16 = (1u16 << GC_ID_BITS) - 1;

/// The general category abbreviations in table id order (id = position + 1).
///
/// The generator writes these ids and the runtime indexes this array, so both must use the same
/// order.
pub const GC_SHORT_NAMES: [&str; 30] = [
    "Cc", "Cf", "Cn", "Co", "Cs", "Ll", "Lm", "Lo", "Lt", "Lu", "Mc", "Me", "Mn", "Nd", "Nl", "No",
    "Pc", "Pd", "Pe", "Pf", "Pi", "Po", "Ps", "Sc", "Sk", "Sm", "So", "Zl", "Zp", "Zs",
];

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

/// Writes `cp` as upper-case hex digits into `out`, returning how many bytes it wrote.
///
/// Shared by the runtime and the trainer so a `*` pattern name materializes identically on
/// both sides: the runtime fills the placeholder in `prefix + hex + suffix` when it composes
/// the name, and the trainer verifies every pattern range with the same format. `width` is a
/// *minimum*, the way Unicode writes the placeholder: `EGYPTIAN HIEROGLYPH-13460` needs five
/// digits where most code points need four. The digit count is clamped to the eight bytes of
/// `out`, so a wider `width` pads with leading zeros up to that bound instead of overflowing.
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

/// The index into [`TONES`] of `cp`, or `None` when `cp` is not a skin-tone component.
fn tone_index(cp: u32) -> Option<usize> {
    TONES.iter().position(|&tone| tone == cp)
}

/// The indices into [`TONES`] of the skin-tone code points in `cps`, in order.
///
/// Returns the indices and their count in a fixed-capacity array, so a lookup with at most two
/// tones does not allocate. Returns `None` as soon as a third tone is seen; callers that need
/// every tone, however many, use [`tone_positions`].
#[must_use]
pub fn tone_indices(cps: impl IntoIterator<Item = u32>) -> Option<([usize; 2], usize)> {
    let mut indices = [0usize; 2];
    let mut count = 0usize;
    for cp in cps {
        let Some(index) = tone_index(cp) else {
            continue;
        };
        *indices.get_mut(count)? = index;
        count += 1;
    }
    Some((indices, count))
}

/// The indices into [`TONES`] of the skin-tone code points in `cps`, in order.
///
/// Unlike [`tone_indices`] this accepts any number of tones, at the cost of allocating.
pub fn tone_positions(cps: impl IntoIterator<Item = u32>) -> Vec<usize> {
    cps.into_iter().filter_map(tone_index).collect()
}

/// Encodes `cps` as UTF-8 into `out`, returning how many bytes it wrote.
///
/// Returns `None` when a code point is not a scalar value or the encoding would overflow `out`.
#[must_use]
pub fn encode_cps(cps: &[u32], out: &mut [u8; MAX_KEY]) -> Option<usize> {
    let mut used = 0usize;
    for &cp in cps {
        let c = char::from_u32(cp)?;
        let end = used.checked_add(c.len_utf8())?;
        c.encode_utf8(out.get_mut(used..end)?);
        used = end;
    }
    Some(used)
}

/// `swap_person` rewrites a `person` base name to the given gender word.
#[must_use]
pub fn swap_person(name: &str, gender: &str) -> String {
    if name == "person" {
        return gender.to_owned();
    }
    if let Some(rest) = name.strip_prefix("person: ") {
        return format!("{gender}: {rest}");
    }
    if let Some(rest) = name.strip_prefix("person ") {
        return format!("{gender} {rest}");
    }
    if let Some(head) = name.strip_suffix(" person") {
        return format!("{head} {gender}");
    }
    format!("{gender} {name}")
}

/// Comma-joins the family words of `cps`, skipping `ignored`.
#[must_use]
pub fn join_family(cps: &[u32], ignored: &[u32]) -> String {
    let mut out = String::new();
    for &cp in cps {
        if ignored.contains(&cp) {
            continue;
        }
        if let Some(word) = family_word(cp) {
            if !out.is_empty() {
                out.push_str(", ");
            }
            out.push_str(word);
        }
    }
    out
}

/// The family-member word of a code point.
#[must_use]
pub const fn family_word(cp: u32) -> Option<&'static str> {
    Some(match cp {
        0x1F9D1 => "adult",
        0x1F9D2 => "child",
        0x1F468 => "man",
        0x1F469 => "woman",
        0x1F466 => "boy",
        0x1F467 => "girl",
        _ => return None,
    })
}

/// The plural word of a pair.
#[must_use]
pub const fn plural(left: u32, right: u32) -> Option<&'static str> {
    Some(match (left, right) {
        (0x1F468, 0x1F468) => "men",
        (0x1F469, 0x1F469) => "women",
        (0x1F9D1, 0x1F9D1) => "people",
        (0x1F469, 0x1F468) => "woman and man",
        _ => return None,
    })
}

/// The phrase a pair's middle code point contributes.
#[must_use]
pub const fn pair_phrase(cp: u32) -> Option<&'static str> {
    Some(match cp {
        0x1F430 => "with bunny ears",
        0x1FAEF => "wrestling",
        0x1F91D => "holding hands",
        _ => return None,
    })
}

/// The keycap word of a base code point.
#[must_use]
pub const fn keycap_word(cp: u32) -> Option<&'static str> {
    Some(match cp {
        0x23 => "NUMBER SIGN",
        0x2A => "ASTERISK",
        0x30 => "DIGIT ZERO",
        0x31 => "DIGIT ONE",
        0x32 => "DIGIT TWO",
        0x33 => "DIGIT THREE",
        0x34 => "DIGIT FOUR",
        0x35 => "DIGIT FIVE",
        0x36 => "DIGIT SIX",
        0x37 => "DIGIT SEVEN",
        0x38 => "DIGIT EIGHT",
        0x39 => "DIGIT NINE",
        _ => return None,
    })
}

/// The `ucd_mask` rule: selects UCD-name words, optionally hyphenating the first two.
#[must_use]
pub fn ucd_mask_name(name: &str, mask: u8, hyphen: bool) -> String {
    let mut out = String::with_capacity(name.len());
    let mut selected = 0usize;
    for (word_index, word) in name.split(' ').enumerate() {
        if word_index >= 8 || mask >> word_index & 1 == 0 {
            continue;
        }
        if selected > 0 {
            out.push(if hyphen && selected == 1 { '-' } else { ' ' });
        }
        out.extend(word.chars().map(|c| c.to_ascii_lowercase()));
        selected += 1;
    }
    out
}

/// The `clock` rule: `CLOCK FACE <HOUR> OCLOCK` becomes `<hour> o’clock`.
#[must_use]
pub fn clock_name(name: &str) -> Option<String> {
    let mut words = name.split(' ');
    let clock = words.next()?;
    let face = words.next()?;
    let hour = words.next()?;
    let oclock = words.next()?;
    if words.next().is_some() || clock != "CLOCK" || face != "FACE" || oclock != "OCLOCK" {
        return None;
    }
    let mut out = String::with_capacity(hour.len() + " o’clock".len());
    out.extend(hour.chars().map(|c| c.to_ascii_lowercase()));
    out.push_str(" o’clock");
    Some(out)
}

/// Renders `base` with `tones` applied, following the 3-bit rule.
///
/// Rule `1` inserts the tone phrase after the first `": "`, rule `2` appends it, and rules
/// `3`/`4` do the same after collapsing consecutive duplicate tones.
///
/// # Panics
///
/// Panics when a tone index is not a position in [`TONES`]; [`tone_indices`] and
/// [`tone_positions`] only produce valid indices.
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
///
/// # Panics
///
/// Panics when a tone index is not a position in [`TONES`]; [`tone_indices`] and
/// [`tone_positions`] only produce valid indices.
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
    fn tone_indices_handles_zero_one_and_two_tones() {
        let (indices, count) = tone_indices(std::iter::empty()).expect("no tones is valid");
        assert_eq!((indices, count), ([0, 0], 0));
        assert_eq!(tone_indices([0x41, 0x42]), Some(([0, 0], 0)));
        assert_eq!(tone_indices([0x1F3FF]), Some(([4, 0], 1)));
        assert_eq!(
            tone_indices([0x41, 0x1F3FB, 0x42, 0x1F3FD]),
            Some(([0, 2], 2))
        );
    }

    #[test]
    fn tone_indices_rejects_a_third_tone() {
        assert_eq!(
            tone_indices([0x1F3FB, 0x1F3FC, 0x1F3FD]),
            None,
            "the fixed-capacity array only holds two tones"
        );
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
    fn encode_cps_writes_utf8_and_rejects_invalid_scalars() {
        let mut buffer = [0u8; MAX_KEY];
        let used = encode_cps(&[0x64, 0x1F994], &mut buffer).expect("scalars fit");
        assert_eq!(&buffer[..used], "d🦔".as_bytes());
        assert_eq!(encode_cps(&[0xD800], &mut buffer), None);
        let too_long = [0x1F994u32; MAX_CPS + 1];
        assert_eq!(encode_cps(&too_long, &mut buffer), None);
    }

    #[test]
    fn grammar_helpers_reproduce_the_published_names() {
        assert_eq!(swap_person("person", "woman"), "woman");
        assert_eq!(swap_person("person: red hair", "man"), "man: red hair");
        assert_eq!(swap_person("person running", "woman"), "woman running");
        assert_eq!(swap_person("running person", "man"), "running man");
        assert_eq!(swap_person("runner", "woman"), "woman runner");

        assert_eq!(family_word(0x1F469), Some("woman"));
        assert_eq!(family_word(0x1F994), None);
        assert_eq!(plural(0x1F469, 0x1F468), Some("woman and man"));
        assert_eq!(plural(0x1F994, 0x1F994), None);
        assert_eq!(pair_phrase(0x1F91D), Some("holding hands"));
        assert_eq!(keycap_word(0x23), Some("NUMBER SIGN"));
        assert_eq!(keycap_word(0x41), None);

        assert_eq!(
            join_family(&[0x1F469, ZWJ, 0x1F466, ZWJ, 0x1F467], &[ZWJ]),
            "woman, boy, girl"
        );
        assert_eq!(
            join_family(&[0x1F469, ZWJ, 0x2764, ZWJ, 0x1F468], &[ZWJ, 0x2764]),
            "woman, man"
        );
    }

    #[test]
    fn ucd_mask_selects_words_and_hyphenates_the_first_pair() {
        assert_eq!(
            ucd_mask_name("COPYRIGHT SIGN", 0b11, false),
            "copyright sign"
        );
        assert_eq!(
            ucd_mask_name("COPYRIGHT SIGN", 0b11, true),
            "copyright-sign"
        );
        assert_eq!(
            ucd_mask_name("ARABIC LIGATURE SALAM", 0b101, true),
            "arabic-salam"
        );
        // Bits past the eighth word are ignored.
        assert_eq!(ucd_mask_name("A B", 0xFF, false), "a b");
        assert_eq!(ucd_mask_name("A B", 0, false), "");
    }

    #[test]
    fn clock_names_require_the_exact_ucd_spelling() {
        assert_eq!(
            clock_name("CLOCK FACE TWO OCLOCK").as_deref(),
            Some("two o’clock")
        );
        assert_eq!(
            clock_name("CLOCK FACE TWELVE OCLOCK").as_deref(),
            Some("twelve o’clock")
        );
        assert_eq!(clock_name("CLOCK FACE TWO"), None);
        assert_eq!(clock_name("CLOCK HAND TWO OCLOCK"), None);
        assert_eq!(clock_name("CLOCK FACE TWO OCLOCK EXTRA"), None);
    }

    #[test]
    fn the_layout_constants_are_self_consistent() {
        assert_eq!(GC_PAGES, 545);
        assert_eq!(GC_SHORT_NAMES.len(), 30);
        assert!(GC_SHORT_NAMES.len() < 1usize << GC_ID_BITS);
        assert_eq!(1u16 << GC_ID_BITS, GC_ID_MASK + 1);
        assert_eq!(UNICODE_START, 0);
        assert_eq!(UNICODE_LIMIT, UNICODE_END + 1);
        assert_eq!(UNICODE_LIMIT as usize >> GC_SHIFT, GC_PAGES - 1);
        assert_eq!(
            HANGUL_END - HANGUL_START + 1,
            u32::try_from(HANGUL_COUNT).expect("the count fits in u32")
        );
    }
}
