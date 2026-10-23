//! Unicode 18.0.0 character names, general categories and emoji sequence names.
//!
//! The `zeta-unicode-build` crate compiles the vendored UCD files in `data/` into one packed
//! table that is linked into the binary with `include_bytes!`. A lookup walks that table
//! directly: there is no parsing, no allocation for stored names, and no decompression. The
//! general category table is a two-level page structure (a `u16` directory plus `u16` range
//! records), and emoji keys are canonicalized — `U+FE0F` is stripped and skin-tone variants are
//! synthesized from a 3-bit rule — so 1,944 stored keys answer for every one of the 3,972
//! sequences the UCD names.
//!
//! ```
//! use zeta_unicode::{emoji_name, general_category, name, name_or_label};
//!
//! assert_eq!(name('ß').unwrap(), "LATIN SMALL LETTER SHARP S");
//! assert_eq!(general_category('ß').long_name(), "Lowercase Letter");
//! assert_eq!(emoji_name("🏳️‍🌈").unwrap(), "rainbow flag");
//! assert_eq!(name_or_label('\u{f8ff}'), "<private-use-F8FF>");
//! ```

mod gc;
mod names;
mod sections;
mod tables;

use std::borrow::Cow;

pub use gc::GeneralCategory;

/// The Unicode version of the data this crate was built from, as `(major, minor, patch)`.
///
/// `zeta-unicode-build` refuses to build when the vendored files disagree, so this cannot
/// drift from the table without a test failing.
pub const UNICODE_VERSION: (u8, u8, u8) = (18, 0, 0);

/// The size budget for the generated table, in bytes.
///
/// The current encoder does not meet it yet; `table_stays_within_the_size_budget` is the gate
/// that starts enforcing it.
pub const TABLE_BUDGET: usize = 100_000;

/// The upper bound the current reference encoder is held to, well above [`TABLE_BUDGET`]: the
/// compressed encoder that meets the budget has not landed yet.
#[cfg(test)]
const ENCODER_TABLE_LIMIT: usize = 2_000_000;

/// The first Hangul syllable, `HANGUL SYLLABLE GA`.
const HANGUL_START: u32 = 0xAC00;
/// The last Hangul syllable.
const HANGUL_END: u32 = 0xD7A3;

/// Choongbae (leading) jamo short names, in `Jamo_Short_Name` order.
const JAMO_LEADING: [&str; 19] = [
    "G", "GG", "N", "D", "DD", "R", "M", "B", "BB", "S", "SS", "", "J", "JJ", "C", "K", "T", "P",
    "H",
];
/// Vowel (medial) jamo short names.
const JAMO_VOWEL: [&str; 21] = [
    "A", "AE", "YA", "YAE", "EO", "E", "YEO", "YE", "O", "WA", "WAE", "OE", "YO", "U", "WEO", "WE",
    "WI", "YU", "EU", "YI", "I",
];
/// Trailing jamo short names, index 0 being "no trailing consonant".
const JAMO_TRAILING: [&str; 28] = [
    "", "G", "GG", "GS", "N", "NJ", "NH", "D", "L", "LG", "LM", "LB", "LS", "LT", "LP", "LH", "M",
    "B", "BS", "S", "SS", "NG", "J", "C", "K", "T", "P", "H",
];

/// The general category abbreviations in table id order (id = position + 1).
///
/// Keep in sync with `zeta-unicode-build`, which emits ids in this order; the round-trip tests
/// walk every range of the vendored `DerivedGeneralCategory.txt` and fail if the two ever
/// disagree.
const GC_SHORT_NAMES: [&str; 30] = [
    "Cc", "Cf", "Cn", "Co", "Cs", "Ll", "Lm", "Lo", "Lt", "Lu", "Mc", "Me", "Mn", "Nd", "Nl", "No",
    "Pc", "Pd", "Pe", "Pf", "Pi", "Po", "Ps", "Sc", "Sk", "Sm", "So", "Zl", "Zp", "Zs",
];

/// `cp >> GC_SHIFT` selects the page of the general category table.
const GC_SHIFT: u32 = 11;

/// The five emoji skin-tone code points, in tone order.
const TONES: [u32; 5] = [0x1F3FB, 0x1F3FC, 0x1F3FD, 0x1F3FE, 0x1F3FF];
/// The CLDR names of the five skin tones, in the same order.
const TONE_NAMES: [&str; 5] = [
    "light skin tone",
    "medium-light skin tone",
    "medium skin tone",
    "medium-dark skin tone",
    "dark skin tone",
];
/// Longest canonical emoji key in bytes; `zeta-unicode-build` asserts no stored key is longer.
const MAX_KEY: usize = 64;

/// Returns the `Name` property of `c`, or the derived name Unicode defines algorithmically.
///
/// Stored names come back borrowed; names built from a pattern (`CJK UNIFIED IDEOGRAPH-4E00`)
/// or from the Hangul jamo tables are allocated.
#[must_use]
pub fn name(c: char) -> Option<Cow<'static, str>> {
    if let Some(stored) = stored_name(c) {
        return Some(Cow::Borrowed(stored));
    }
    let mut buf = String::new();
    if derived_name(c, &mut buf) {
        return Some(Cow::Owned(buf));
    }
    None
}

/// Appends the name of `c` to `buf`, returning `false` when `c` has no name.
///
/// This is the zero-allocation path: stored names are copied straight out of the table and
/// derived names are formatted into `buf`.
///
/// # Examples
///
/// ```
/// let mut buf = String::new();
/// assert!(zeta_unicode::write_name('中', &mut buf));
/// assert_eq!(buf, "CJK UNIFIED IDEOGRAPH-4E2D");
/// ```
#[must_use = "the return value reports whether `c` has a name"]
pub fn write_name(c: char, buf: &mut String) -> bool {
    if let Some(stored) = stored_name(c) {
        buf.push_str(stored);
        return true;
    }
    derived_name(c, buf)
}

/// Returns the name of `c`, or the label Unicode defines for code points without one.
///
/// Labels are `<private-use-F8FF>`, `<reserved-0378>`, `<noncharacter-FFFE>` and
/// `<control-0000>`; they are rendered on the spot because storing them would cost more than
/// the six bytes they take.
#[must_use]
pub fn name_or_label(c: char) -> Cow<'static, str> {
    if let Some(name) = name(c) {
        return name;
    }

    let cp = u32::from(c);
    let kind = match general_category(c) {
        GeneralCategory::PrivateUse => "private-use",
        GeneralCategory::Control => "control",
        GeneralCategory::Surrogate => "surrogate",
        GeneralCategory::Unassigned if is_noncharacter(cp) => "noncharacter",
        _ => "reserved",
    };
    Cow::Owned(format!("<{kind}-{cp:04X}>"))
}

/// Returns the general category of `c`; unassigned points answer with
/// [`GeneralCategory::Unassigned`].
#[must_use]
pub fn general_category(c: char) -> GeneralCategory {
    general_category_of(u32::from(c)).unwrap_or(GeneralCategory::Unassigned)
}

/// Returns the CLDR short name of an emoji sequence such as `🏳️‍🌈`.
///
/// Names come from `emoji-test.txt`, or from `NamedSequences.txt` for the official name of an
/// emoji sequence such as a keycap. Non-emoji named sequences such as Latin letters with
/// combining marks are not part of this table.
///
/// Keys are canonicalized: `U+FE0F` is optional (`🚶‍♂` answers like `🚶‍♂️`) and skin-tone
/// variants resolve by synthesizing the base name with the recorded formatting rule. The name
/// is returned as published — lower case for emoji — so callers can case it however their
/// reply needs. Returns `None` for anything that is not a known sequence, including non-emoji
/// text.
#[must_use]
pub fn emoji_name(sequence: &str) -> Option<Cow<'static, str>> {
    emoji_name_of(sequence)
}

/// Is `cp` one of Unicode's noncharacters (`U+FDD0..U+FDEF` or `*FFFE`/`*FFFF`)?
#[must_use]
pub const fn is_noncharacter(cp: u32) -> bool {
    matches!(cp, 0xFDD0..=0xFDEF) || matches!(cp & 0xFFFF, 0xFFFE | 0xFFFF)
}

/// Writes `cp` as upper-case hex digits into `out`, returning how many bytes it wrote.
///
/// The shared hole filler for `*` name patterns and for the generated rule tree, so both fill a
/// hole the same way. `width` is a *minimum*, the way Unicode writes the placeholder:
/// `EGYPTIAN HIEROGLYPH-13460` needs five digits where most code points need four.
fn write_hex(out: &mut [u8; 8], cp: u32, width: u8) -> usize {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut needed = 0usize;
    let mut rest = cp;
    while rest > 0 {
        rest >>= 4;
        needed += 1;
    }
    let count = needed.max(usize::from(width.min(8)));
    if count == 0 {
        return 0;
    }
    let mut shift = u32::try_from(count - 1).expect("at most eight digits") * 4;
    for slot in out.iter_mut().take(count) {
        *slot = DIGITS[((cp >> shift) & 0xF) as usize];
        shift = shift.saturating_sub(4);
    }
    count
}

/// [`write_hex`] for the generated rule tree, which appends into the decoded name buffer.
///
/// The `*` patterns reach it through [`pattern_name`]; the tree keeps the two filling a hole the
/// same way.
fn push_hex(out: &mut Vec<u8>, cp: u32, width: u8) {
    let mut digits = [0u8; 8];
    let used = write_hex(&mut digits, cp, width);
    out.extend_from_slice(&digits[..used]);
}

/// Looks up a stored (non-derived) name.
fn stored_name(c: char) -> Option<&'static str> {
    names::name_for(u32::from(c))
}

/// Formats a derived name (an `*` pattern or a Hangul syllable) into `buf`.
///
/// Returns `false` when `c` has no name at all.
fn derived_name(c: char, buf: &mut String) -> bool {
    let cp = u32::from(c);

    if let Some(pattern) = pattern_name(cp) {
        buf.push_str(&pattern);
        return true;
    }

    if (HANGUL_START..=HANGUL_END).contains(&cp) {
        let index = cp - HANGUL_START;
        let leading = JAMO_LEADING[(index / 588) as usize];
        let vowel = JAMO_VOWEL[((index % 588) / 28) as usize];
        let trailing = JAMO_TRAILING[(index % 28) as usize];
        buf.push_str("HANGUL SYLLABLE ");
        buf.push_str(leading);
        buf.push_str(vowel);
        buf.push_str(trailing);
        return true;
    }

    false
}

/// Finds the `*` pattern covering `cp` and renders it, prefix and suffix included.
///
/// Records are `(start, end)` followed by the two halves of Unicode's `*` pattern, so the name
/// is `prefix + hex(cp) + suffix`. They are stored plainly: there are twenty of them and a name
/// miss reads them on every call.
fn pattern_name(cp: u32) -> Option<String> {
    let records = tables::section(sections::NAME_PATTERNS)?;
    let mut at = 0usize;
    while at < records.len() {
        let start = tables::u32_at(records, at)?;
        let end = tables::u32_at(records, at + 4)?;
        let prefix_len = usize::from(tables::u16_at(records, at + 8)?);
        let prefix = tables::bytes_at(records, at + 10, prefix_len)?;
        let suffix_len_at = at + 10 + prefix_len;
        let suffix_len = usize::from(tables::u16_at(records, suffix_len_at)?);
        let suffix = tables::bytes_at(records, suffix_len_at + 2, suffix_len)?;
        at = suffix_len_at + 2 + suffix_len;

        if cp < start {
            return None;
        }
        if cp <= end {
            let mut name = Vec::with_capacity(prefix.len() + suffix.len() + 8);
            name.extend_from_slice(prefix);
            push_hex(&mut name, cp, 0);
            name.extend_from_slice(suffix);
            return String::from_utf8(name).ok();
        }
    }
    None
}

/// Looks up the general category of `cp` in the two-level page table.
fn general_category_of(cp: u32) -> Option<GeneralCategory> {
    const GC_PAGES: usize = (0x0011_0000_usize >> GC_SHIFT) + 1;

    let table = tables::section(sections::GC)?;
    let directory = tables::bytes_at(table, 0, GC_PAGES * 2)?;
    let records = table.get(GC_PAGES * 2..).unwrap_or_default();

    let page = usize::try_from(cp >> GC_SHIFT).ok()?;
    let relative = cp - (u32::try_from(page).ok()? << GC_SHIFT);
    let first = usize::from(tables::u16_at(directory, page * 2)?);
    let end = usize::from(tables::u16_at(directory, page * 2 + 2)?);

    // The run ending before `first` may carry into this page — because the page has no run
    // starting in it (`first == end`), or because `cp` falls before the page's first run —
    // otherwise find the last run in the page whose start is at or before `cp`.
    let record = if first == end || relative < u32::from(tables::u16_at(records, first * 2)? >> 5) {
        first.checked_sub(1)?
    } else {
        let (mut low, mut high) = (first, end);
        while low < high {
            let mid = low + (high - low) / 2;
            if u32::from(tables::u16_at(records, mid * 2)? >> 5) <= relative {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low.checked_sub(1)?
    };

    let id = usize::from(tables::u16_at(records, record * 2)? & 31).checked_sub(1)?;
    let short: [u8; 2] = GC_SHORT_NAMES.get(id)?.as_bytes().try_into().ok()?;
    GeneralCategory::from_short_name(&short)
}

/// The emoji section, split into the blobs and offset tables it stores.
struct Emoji<'a> {
    keys: &'a [u8],
    key_offsets: &'a [u8],
    names: &'a [u8],
    name_offsets: &'a [u8],
    codes: &'a [u8],
    entries: usize,
}

impl<'a> Emoji<'a> {
    /// Splits the section payload into its parts: entry count, key blob with `u16` offsets,
    /// name blob with `u16` offsets, and the packed 3-bit tone rules.
    fn section() -> Option<Self> {
        let section = tables::section(sections::EMOJI)?;
        let entries = usize::from(tables::u16_at(section, 0)?);
        let key_bytes = usize::from(tables::u16_at(section, 2)?);
        let name_bytes = usize::from(tables::u16_at(section, 4)?);
        let code_bytes = usize::from(tables::u16_at(section, 6)?);
        let offsets = (entries + 1) * 2;
        Some(Self {
            keys: tables::bytes_at(section, 8, key_bytes)?,
            key_offsets: tables::bytes_at(section, 8 + key_bytes, offsets)?,
            names: tables::bytes_at(section, 8 + key_bytes + offsets, name_bytes)?,
            name_offsets: tables::bytes_at(section, 8 + key_bytes + offsets + name_bytes, offsets)?,
            codes: tables::bytes_at(
                section,
                8 + key_bytes + name_bytes + 2 * offsets,
                code_bytes,
            )?,
            entries,
        })
    }

    /// Returns key `index` out of the key blob, using the `u16` offset table.
    fn key(&self, index: usize) -> Option<&'a [u8]> {
        let start = usize::from(tables::u16_at(self.key_offsets, index * 2)?);
        let end = usize::from(tables::u16_at(self.key_offsets, index * 2 + 2)?);
        tables::bytes_at(self.keys, start, end.checked_sub(start)?)
    }

    /// Returns name `index` out of the name blob, using the `u16` offset table.
    fn name(&self, index: usize) -> Option<&'a [u8]> {
        let start = usize::from(tables::u16_at(self.name_offsets, index * 2)?);
        let end = usize::from(tables::u16_at(self.name_offsets, index * 2 + 2)?);
        tables::bytes_at(self.names, start, end.checked_sub(start)?)
    }

    /// Returns the 3-bit tone formatting rule of entry `index`.
    ///
    /// The rule can straddle a byte boundary, which is why two bytes are read and the value
    /// rotated into place.
    fn tone_code(&self, index: usize) -> Option<u8> {
        let bit = index * 3;
        let byte = self.codes.get(bit / 8)?;
        let next = self.codes.get(bit / 8 + 1).copied().unwrap_or(0);
        let value = (u16::from(*byte) | (u16::from(next) << 8)) >> (bit % 8);
        Some((value & 7) as u8)
    }

    /// Binary-searches the stored keys for `needle`.
    fn find(&self, needle: &[u8]) -> Option<usize> {
        let (mut low, mut high) = (0usize, self.entries);
        while low < high {
            let mid = low + (high - low) / 2;
            match needle.cmp(self.key(mid)?) {
                std::cmp::Ordering::Less => high = mid,
                std::cmp::Ordering::Greater => low = mid + 1,
                std::cmp::Ordering::Equal => return Some(mid),
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
        let tones = tone_positions(text);
        if tones.is_empty() {
            return None;
        }

        let base = strip_tones(text, scratch)?;
        let index = self.find(base)?;
        let code = self.tone_code(index)?;
        if code == 0 {
            return None;
        }
        let base_name = std::str::from_utf8(self.name(index)?).ok()?;
        Some(Cow::Owned(format_toned(code, base_name, &tones)))
    }
}

/// Looks up the emoji entry whose canonical key equals `sequence`.
fn emoji_name_of(sequence: &str) -> Option<Cow<'static, str>> {
    let emoji = Emoji::section()?;
    let mut buffer = [0u8; MAX_KEY];
    let mut scratch = [0u8; MAX_KEY];
    let needle = canonical_key(sequence.as_bytes(), &mut buffer)?;

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
        let byte = *key.get(index)?;
        *buffer.get_mut(used)? = byte;
        used += 1;
        index += 1;
    }
    buffer.get(..used)
}

/// Indices (into [`TONES`]) of the skin-tone characters in `text`, in order.
fn tone_positions(text: &str) -> Vec<usize> {
    text.chars()
        .filter_map(|c| TONES.iter().position(|&tone| tone == u32::from(c)))
        .collect()
}

/// Copies `text` into `buffer` without its skin-tone characters, so the tone fallback needs no
/// allocation.
fn strip_tones<'a>(text: &str, buffer: &'a mut [u8; MAX_KEY]) -> Option<&'a [u8]> {
    let mut used = 0usize;
    for c in text.chars() {
        if TONES.contains(&u32::from(c)) {
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

/// Renders `base` with `tones` applied, following the 3-bit rule: `1` inserts the tone phrase
/// after the first `": "`, `2` appends it, `3`/`4` do the same after collapsing consecutive
/// duplicate tones.
fn format_toned(code: u8, base: &str, tones: &[usize]) -> String {
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
fn tone_phrase(code: u8, tones: &[usize]) -> String {
    let mut tone_list = tones.to_vec();
    if code == 3 || code == 4 {
        tone_list.dedup();
    }
    tone_list
        .iter()
        .map(|&index| TONE_NAMES[index])
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    const DERIVED_NAME: &str = include_str!("../data/DerivedName.txt");
    const DERIVED_GC: &str = include_str!("../data/DerivedGeneralCategory.txt");
    const NAME_ALIASES: &str = include_str!("../data/NameAliases.txt");
    const EMOJI_TEST: &str = include_str!("../data/emoji-test.txt");
    const NAMED_SEQUENCES: &str = include_str!("../data/NamedSequences.txt");

    #[test]
    fn unicode_version_matches_the_vendored_data() {
        let header = DERIVED_NAME.lines().next().expect("header line");
        let version = header
            .trim_start_matches("# DerivedName-")
            .trim_end_matches(".txt");
        let parsed: Vec<u8> = version
            .split('.')
            .map(|part| part.parse().expect("numeric version component"))
            .collect();
        assert_eq!(
            (parsed[0], parsed[1], parsed[2]),
            UNICODE_VERSION,
            "UNICODE_VERSION is stale"
        );
        assert_eq!(
            tables::version(),
            UNICODE_VERSION,
            "the generated table was built from a different Unicode release"
        );
    }

    #[test]
    fn every_name_in_the_ucd_round_trips() {
        let mut checked = 0usize;

        for line in DERIVED_NAME
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let (lhs, rhs) = line.split_once(';').expect("field separator");
            let lhs = lhs.trim();

            // `DerivedName.txt` spells a pattern as a `*` placeholder "in place of the code
            // point in hex", on a single-code-point line as well as on a range: U+18CFF is the
            // one that matters, since it is the only point whose line carries a `*`.
            let value = rhs.trim();
            let pattern = value.split_once('*');
            if let Some((prefix, _width)) = pattern {
                let range = lhs
                    .split_once("..")
                    .map_or_else(|| (lhs, lhs), |(start, end)| (start, end));
                let start = u32::from_str_radix(range.0, 16).expect("range start");
                let end = u32::from_str_radix(range.1, 16).expect("range end");
                for cp in start..=end {
                    let Some(c) = char::from_u32(cp) else {
                        continue;
                    };
                    assert_eq!(
                        name(c).as_deref(),
                        Some(format!("{prefix}{cp:04X}").as_str()),
                        "pattern name for U+{cp:04X}"
                    );
                    checked += 1;
                }
                continue;
            }

            let cp = u32::from_str_radix(lhs, 16).expect("code point");
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            assert_eq!(name(c).as_deref(), Some(rhs.trim()), "name for U+{cp:04X}");
            checked += 1;
        }

        assert!(checked > 150_000, "only checked {checked} names");
    }

    #[test]
    fn alias_backed_names_are_available() {
        let mut checked = 0usize;

        for line in NAME_ALIASES
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let cp = u32::from_str_radix(line.split(';').next().expect("code point"), 16)
                .expect("code point");
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            assert!(
                name(c).is_some(),
                "U+{cp:04X} has an alias but answered with no name"
            );
            checked += 1;
        }

        assert!(checked > 300, "only checked {checked} aliases");
        assert_eq!(name('\u{16}').as_deref(), Some("SYNCHRONOUS IDLE"));
        assert_eq!(name('\u{9}').as_deref(), Some("CHARACTER TABULATION"));
    }

    #[test]
    fn general_categories_match_the_ucd() {
        let mut checked = 0usize;

        for line in DERIVED_GC
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let (lhs, rest) = line.split_once(';').expect("field separator");
            let letter = rest.split('#').next().expect("category").trim();
            let lhs = lhs.trim();
            let (start, end) = match lhs.split_once("..") {
                Some((start, end)) => (start, end),
                None => (lhs, lhs),
            };
            let start = u32::from_str_radix(start, 16).expect("range start");
            let end = u32::from_str_radix(end, 16).expect("range end");

            for cp in [start, end, start + (end - start) / 2] {
                let Some(c) = char::from_u32(cp) else {
                    continue;
                };
                assert_eq!(
                    general_category(c).short_name(),
                    letter,
                    "general category of U+{cp:04X}"
                );
                checked += 1;
            }
        }

        assert!(checked > 10_000, "only checked {checked} ranges");
        assert_eq!(general_category('A'), GeneralCategory::UppercaseLetter);
        assert_eq!(general_category('あ'), GeneralCategory::OtherLetter);
        assert_eq!(general_category('\u{378}'), GeneralCategory::Unassigned);
    }

    /// Rebuilds the emoji oracle the way `zeta-unicode-build` does: canonical keys (U+FE0F removed) with
    /// named-sequence names taking precedence over fully-qualified, then qualified over
    /// unqualified.
    fn emoji_oracle() -> BTreeMap<String, String> {
        let mut ranked: BTreeMap<Vec<u32>, (u8, String)> = BTreeMap::new();

        for line in EMOJI_TEST
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let (left, rest) = line.split_once(';').expect("field separator");
            let (status, comment) = rest.split_once('#').expect("comment");
            let cps: Vec<u32> = left
                .split_whitespace()
                .map(|cp| u32::from_str_radix(cp, 16).expect("code point"))
                .collect();
            let rank = match status.trim() {
                "fully-qualified" | "component" => 2,
                "minimally-qualified" => 1,
                "unqualified" => 0,
                other => panic!("unknown status {other}"),
            };
            let words: Vec<&str> = comment.split_whitespace().collect();
            match ranked.get(&cps) {
                Some((existing, _)) if *existing > rank => {}
                _ => {
                    ranked.insert(cps, (rank, words[2..].join(" ")));
                }
            }
        }

        // `NamedSequences.txt` also carries non-emoji sequences (Latin letters with combining
        // marks); the build drops those, so the oracle must too.
        let emoji_keys: BTreeSet<Vec<u32>> = ranked
            .keys()
            .map(|cps| cps.iter().copied().filter(|&cp| cp != 0xFE0F).collect())
            .collect();

        for line in NAMED_SEQUENCES
            .lines()
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let (name, cps) = line.split_once(';').expect("field separator");
            let cps: Vec<u32> = cps
                .split_whitespace()
                .map(|cp| u32::from_str_radix(cp, 16).expect("code point"))
                .collect();
            let canonical: Vec<u32> = cps.iter().copied().filter(|&cp| cp != 0xFE0F).collect();
            if !emoji_keys.contains(&canonical) {
                continue;
            }
            ranked.insert(cps, (3, name.trim().to_owned()));
        }

        let mut oracle = BTreeMap::new();
        for (cps, (_, name)) in ranked {
            let key: String = cps
                .iter()
                .filter_map(|&cp| char::from_u32(cp).filter(|c| u32::from(*c) != 0xFE0F))
                .collect();
            oracle.insert(key, name);
        }
        oracle
    }

    #[test]
    fn emoji_sequences_resolve_to_their_names() {
        let oracle = emoji_oracle();

        for (key, name) in &oracle {
            assert_eq!(
                emoji_name(key).as_deref(),
                Some(name.as_str()),
                "emoji name for {key:?}"
            );
        }

        assert!(
            // 5,244 emoji-test entries collapse onto 3,972 canonical keys (U+FE0F merged
            // away); the keycap named sequences only supply names for existing keys.
            oracle.len() > 3_900,
            "only checked {} sequences",
            oracle.len()
        );
        assert_eq!(emoji_name("🏳️‍🌈").as_deref(), Some("rainbow flag"));
        assert_eq!(emoji_name("2️⃣").as_deref(), Some("KEYCAP DIGIT TWO"));
        // Text presentation answers exactly like emoji presentation.
        assert_eq!(emoji_name("🚶‍♂").as_deref(), Some("man walking"));
        // A skin-tone variant is synthesized from its base and the 3-bit rule.
        assert_eq!(
            emoji_name("🫶🏻").as_deref(),
            Some("heart hands: light skin tone")
        );
        assert_eq!(
            emoji_name("🖐🏻").as_deref(),
            Some("hand with fingers splayed: light skin tone")
        );
        assert_eq!(emoji_name("d'être"), None);
        assert_eq!(emoji_name("🐱‍🏍"), None);
        // A non-emoji named sequence (a decomposed Latin letter) is not an emoji.
        assert_eq!(emoji_name("\u{0100}\u{0300}"), None);
    }

    #[test]
    fn unnamed_code_points_get_a_label() {
        assert_eq!(name_or_label('\u{f8ff}'), "<private-use-F8FF>");
        assert_eq!(name_or_label('\u{378}'), "<reserved-0378>");
        assert_eq!(name_or_label('\u{FFFE}'), "<noncharacter-FFFE>");
        assert_eq!(name('\u{0}').as_deref(), Some("NULL"));
    }

    #[test]
    fn write_name_matches_name() {
        let mut buf = String::new();
        assert!(write_name('中', &mut buf));
        assert_eq!(buf, "CJK UNIFIED IDEOGRAPH-4E2D");

        buf.clear();
        assert!(write_name('가', &mut buf));
        assert_eq!(buf, "HANGUL SYLLABLE GA");

        buf.clear();
        assert!(!write_name('\u{378}', &mut buf));
        assert_eq!(buf, "");
    }

    #[test]
    fn write_hex_handles_the_boundaries() {
        let mut digits = [0u8; 8];
        assert_eq!(write_hex(&mut digits, 0, 0), 0);
        assert_eq!(write_hex(&mut digits, 0, 4), 4);
        assert_eq!(&digits[..4], b"0000");
        assert_eq!(write_hex(&mut digits, 0x4E2D, 0), 4);
        assert_eq!(&digits[..4], b"4E2D");
        // `EGYPTIAN HIEROGLYPH-13460` needs five digits where the width asks for four.
        assert_eq!(write_hex(&mut digits, 0x13460, 4), 5);
        assert_eq!(&digits[..5], b"13460");
    }

    /// The generator must reject keys the decoder's fixed scratch cannot hold, so the two
    /// constants are pinned equal: a drift would make an overlong key a silent emoji miss
    /// instead of a build failure.
    const _: () = assert!(MAX_KEY == zeta_unicode_build::MAX_KEY);

    /// Prints every section's size, then holds the table to [`ENCODER_TABLE_LIMIT`]: the budget
    /// itself is not reachable until the compressed encoder replaces the uncompressed reference
    /// one this crate still ships.
    #[test]
    fn table_stays_within_the_size_budget() {
        let measured = [
            ("names", sections::NAMES),
            ("name runs", sections::NAME_RUNS),
            ("name patterns", sections::NAME_PATTERNS),
            ("general category", sections::GC),
            ("emoji", sections::EMOJI),
        ];
        for (label, id) in measured {
            let length = tables::section(id).map_or(0, <[u8]>::len);
            eprintln!("{label:>18}: {length:>9} bytes");
        }
        eprintln!(
            "{:>18}: {:>9} bytes (budget {})",
            "total",
            tables::TABLE.len(),
            TABLE_BUDGET
        );

        // The v0 reference encoder is deliberately uncompressed, so the limit it is held to is
        // the encoder's, not the budget's.
        assert!(
            tables::TABLE.len() <= ENCODER_TABLE_LIMIT,
            "table grew to {} bytes",
            tables::TABLE.len()
        );
    }

    /// Times every public lookup over its whole domain, so a layout change can be compared on
    /// both architectures:
    ///
    /// ```sh
    /// cargo test -p zeta-unicode --release -- --ignored --nocapture benchmarks
    /// ```
    #[test]
    #[ignore = "benchmark"]
    fn benchmarks() {
        use std::time::{Duration, Instant};

        /// Named characters per name run, and emoji probes per emoji run.
        const SAMPLES: u32 = 1 << 16;
        /// Every code point in the Unicode range.
        const ALL_CODE_POINTS: u32 = 0x0011_0000;

        fn per_lookup(elapsed: Duration, samples: u32) -> f64 {
            elapsed.as_secs_f64() * 1e9 / f64::from(samples)
        }

        // Every third scalar value below U+3000: dense in the named ranges, and it skips the
        // surrogates `char::from_u32` rejects.
        let points: Vec<char> = (0..SAMPLES)
            .filter_map(|i| char::from_u32(0x20 + i))
            .filter(|c| u32::from(*c) < 0x3000)
            .collect();
        let named = u32::try_from(points.len()).expect("fewer than u32::MAX characters");

        // Three warm-up rounds first: the opening passes pay the page faults.
        for round in 0..4 {
            let start = Instant::now();
            let mut sink = 0u64;
            for c in &points {
                sink += name(*c).map_or(0, |found| found.len() as u64);
            }
            if round == 3 {
                eprintln!(
                    "name:             {:>8.2} ns/lookup (sink {sink})",
                    per_lookup(start.elapsed(), named)
                );
            }
        }

        for round in 0..4 {
            let start = Instant::now();
            let mut sink = 0u64;
            for cp in 0..ALL_CODE_POINTS {
                let c = char::from_u32(cp).unwrap_or(' ');
                sink += general_category(c).short_name().len() as u64;
            }
            if round == 3 {
                eprintln!(
                    "general_category: {:>8.2} ns/lookup (sink {sink})",
                    per_lookup(start.elapsed(), ALL_CODE_POINTS)
                );
            }
        }

        // The interesting emoji shapes: canonical hit, named sequence, synthesized tone
        // variant, tone fallback with two tones, a miss, and a long tag sequence.
        let probes = ["🏳️‍🌈", "2️⃣", "👨🏻‍❤️‍💋‍👨🏻", "🫶🏻", "🧑🏽‍🦰", "🐱‍🏍", "d'être", "🏴󠁧󠁢󠁥󠁮󠁧󠁿"];
        let probes: u32 = u32::try_from(probes.len()).expect("a handful of probes");
        for round in 0..4 {
            let start = Instant::now();
            let mut sink = 0u64;
            for i in 0..SAMPLES {
                for probe in ["🏳️‍🌈", "2️⃣", "👨🏻‍❤️‍💋‍👨🏻", "🫶🏻", "🧑🏽‍🦰", "🐱‍🏍", "d'être", "🏴󠁧󠁢󠁥󠁮󠁧󠁿"]
                {
                    sink += emoji_name(probe).map_or(0, |found| found.len() as u64);
                    std::hint::black_box(i);
                }
            }
            if round == 3 {
                eprintln!(
                    "emoji_name:       {:>8.2} ns/lookup (sink {sink})",
                    per_lookup(start.elapsed(), SAMPLES * probes)
                );
            }
        }
    }
}
