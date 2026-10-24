//! Nomina emoji lookup: canonical keys, binary search and rule-generated names.
//!
//! The section stores the 1,944 canonical entries of the shipped table in four Huffman-coded
//! streams. [`Emoji::parse`] decodes them once into a UTF-8 key arena with `u16` starts, a
//! leaked literal-name arena, and one packed `u16` per entry holding the rule id (low four bits)
//! and its argument (high twelve bits). A lookup canonicalizes `U+FE0F`, binary-searches the key
//! arena, and materializes the name: a literal is borrowed, a rule name is built. Skin-tone
//! variants keep the incumbent behaviour: strip the tones, find the base, and replay the base's
//! 3-bit formatting rule when the 2-bit published-tone mask allows it.
//!
//! The `ucd_mask` and `clock` rules need the character name of one code point. [`NameLookup`]
//! is the seam for that: the name layer supplies it, so this module never depends on the name
//! decoder.
//!
//! Literal names are borrowed from a leaked arena; a name a rule generates is allocated on
//! every lookup, so a ruled entry returns [`Cow::Owned`] where the incumbent table returned a
//! borrowed stored string. The text is identical; only the `Cow` variant differs.

use std::borrow::Cow;
use std::cmp::Ordering;

use zeta_unicode_format::{MAX_KEY, TONE_NAMES, TONES, format_toned, is_tone, tone_indices};

/// The number of grammar rules, literal included.
const RULE_COUNT: usize = 14;
/// Rule id `literal`: the name is in the literal-name arena.
const RULE_LITERAL: u8 = 0;
/// Rule id `ucd_mask`: select words from the lowercase UCD name.
const RULE_UCD_MASK: u8 = 1;
/// Rule id `ucd_mask_hyphen`: select words, joining the first two with a hyphen.
const RULE_UCD_MASK_HYPHEN: u8 = 2;
/// Rule id `keycap`: `KEYCAP ` plus the keycap word.
const RULE_KEYCAP: u8 = 3;
/// Rule id `gender_w`: swap `person` for `woman` in a base name.
const RULE_GENDER_WOMAN: u8 = 4;
/// Rule id `gender_m`: swap `person` for `man` in a base name.
const RULE_GENDER_MAN: u8 = 5;
/// Rule id `facing`: base name plus ` facing right`.
const RULE_FACING: u8 = 6;
/// Rule id `family`: `family: ` plus comma-joined family words.
const RULE_FAMILY: u8 = 7;
/// Rule id `couple`: `couple with heart: ` plus comma-joined family words.
const RULE_COUPLE: u8 = 8;
/// Rule id `kiss`: `kiss: ` plus comma-joined family words.
const RULE_KISS: u8 = 9;
/// Rule id `pair`: plural word plus the pair phrase.
const RULE_PAIR: u8 = 10;
/// Rule id `tone_component`: one of the five skin-tone names.
const RULE_TONE_COMPONENT: u8 = 11;
/// Rule id `clock`: the hour word plus ` o’clock`.
const RULE_CLOCK: u8 = 12;
/// Rule id `keycap10`: the literal name `keycap: 10`.
const RULE_KEYCAP_10: u8 = 13;

/// The most code points one stored key may have.
const MAX_CPS: usize = 16;
/// Four stream descriptors of `u16 raw_len | u16 coded_len | u8 order`.
const STREAM_COUNT: usize = 4;

/// The `U+200D` zero-width joiner.
const ZWJ: u32 = 0x200D;

/// Supplies the Unicode character name of one code point to the emoji grammar.
///
/// The name must be the `Name` property as `DerivedName.txt` spells it (for example
/// `COPYRIGHT SIGN`), not a `NameAliases.txt` alias or a `<control-…>` label: the
/// `ucd_mask` and `clock` rules were verified against that spelling.
///
/// Implemented for any `FnMut(u32, &mut String) -> bool`; a closure can forward to the crate's
/// name decoder without this module depending on it.
pub trait NameLookup {
    /// Appends the name of `cp` to `out`, returning `false` when the code point has no name.
    fn write_name(&mut self, cp: u32, out: &mut String) -> bool;
}

impl<F> NameLookup for F
where
    F: FnMut(u32, &mut String) -> bool,
{
    fn write_name(&mut self, cp: u32, out: &mut String) -> bool {
        self(cp, out)
    }
}

/// The decoded emoji section.
pub struct Emoji {
    /// Every canonical key, back to back and sorted by UTF-8 bytes.
    keys: Vec<u8>,
    /// Start offset of each key, with the arena length appended.
    key_offsets: Vec<u16>,
    /// The literal names, concatenated and validated as UTF-8.
    names: &'static str,
    /// Start offset of each literal name, with the arena length appended.
    name_offsets: Vec<u16>,
    /// One packed `(argument << 4) | rule id` per entry.
    entries: Vec<u16>,
    /// Packed 3-bit tone formatting rules.
    tone_codes: Vec<u8>,
    /// Packed 2-bit published-tone-count masks.
    tone_masks: Vec<u8>,
    /// One bit per possible first key byte, for the cheap admission check.
    first_bytes: [u64; 4],
}

/// The parsed emoji section, initialized by the caller on first use.
impl Emoji {
    /// Parses and decodes an emoji section.
    ///
    /// Returns `None` for a truncated or inconsistent section; every lookup on a parsed table is
    /// total. The literal-name arena is leaked once, so literal names can come back as
    /// `&'static str`.
    #[must_use]
    pub fn parse(section: &[u8]) -> Option<Self> {
        let count = usize::from(u16_at(section, 0)?);
        let literal_count = usize::from(u16_at(section, 2)?);
        if literal_count > count {
            return None;
        }
        let mut descriptors = Vec::with_capacity(STREAM_COUNT);
        let mut at = 4usize;
        for _ in 0..STREAM_COUNT {
            let raw_len = usize::from(u16_at(section, at)?);
            let coded_len = usize::from(u16_at(section, at + 2)?);
            let order = *section.get(at + 4)?;
            if order > 1 {
                return None;
            }
            descriptors.push((raw_len, coded_len, order));
            at += 5;
        }
        let mut streams = Vec::with_capacity(STREAM_COUNT);
        for &(raw_len, coded_len, order) in &descriptors {
            let coded = section.get(at..at.checked_add(coded_len)?)?;
            streams.push(huffman::decode(coded, raw_len, order)?);
            at += coded_len;
        }
        let tone_codes_len = count.checked_mul(3)?.div_ceil(8);
        let tone_masks_len = count.checked_mul(2)?.div_ceil(8);
        let tone_codes = section.get(at..at.checked_add(tone_codes_len)?)?.to_vec();
        at += tone_codes_len;
        let tone_masks = section.get(at..at.checked_add(tone_masks_len)?)?.to_vec();
        at += tone_masks_len;
        if at != section.len() {
            return None;
        }

        let keys_stream = streams.first()?;
        let names_stream = streams.get(1)?;
        let rules_stream = streams.get(2)?;
        let masks_stream = streams.get(3)?;
        if rules_stream.len() != count {
            return None;
        }

        let (keys, key_offsets) = decode_keys(keys_stream, count)?;
        let (names, name_offsets) = decode_names(names_stream, literal_count)?;
        let (entries, mask_count) = decode_entries(rules_stream, masks_stream, literal_count)?;
        if mask_count != masks_stream.len() {
            return None;
        }

        let mut first_bytes = [0u64; 4];
        for pair in key_offsets.windows(2) {
            let start = usize::from(pair[0]);
            let end = usize::from(pair[1]);
            let key = keys.get(start..end)?;
            let first = *key.first()?;
            first_bytes[usize::from(first) >> 6] |= 1 << (first & 63);
        }

        Some(Self {
            keys,
            key_offsets,
            names,
            name_offsets,
            entries,
            tone_codes,
            tone_masks,
            first_bytes,
        })
    }

    /// The number of stored canonical entries.
    #[must_use]
    pub const fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether a stored key can start with `byte`.
    ///
    /// This is the incumbent admission check, not a match.
    #[must_use]
    pub fn may_start(&self, byte: u8) -> bool {
        self.first_bytes[usize::from(byte) >> 6] & (1 << (byte & 63)) != 0
    }

    /// Returns the CLDR short name of an emoji sequence such as `🏳️‍🌈`.
    ///
    /// Keys are canonicalized by dropping `U+FE0F`; skin-tone variants resolve by stripping the
    /// tones and replaying the base's formatting rule. Returns `None` for non-emoji text and for
    /// sequences whose base or published tone count is unknown.
    #[must_use]
    pub fn emoji_name<L: NameLookup>(
        &self,
        lookup: &mut L,
        sequence: &str,
    ) -> Option<Cow<'static, str>> {
        let leading = *sequence.as_bytes().first()?;
        let mut chars = sequence.chars();
        let first = chars.next()?;
        // A lone skin-tone component is an entry; a tone or variation selector in front of
        // anything else is not a sequence.
        if chars.next().is_some() && (is_tone(u32::from(first)) || first == '\u{FE0F}') {
            return None;
        }
        if !self.may_start(leading) {
            return None;
        }

        let mut buffer = [0u8; MAX_KEY];
        let needle = canonical_key(sequence.as_bytes(), &mut buffer)?;
        if let Some(index) = self.find(needle) {
            return self.materialize(index, lookup);
        }

        let mut scratch = [0u8; MAX_KEY];
        self.tone_variant(needle, &mut scratch, lookup)
    }

    /// Returns the key of entry `index`.
    fn key(&self, index: usize) -> Option<&[u8]> {
        let start = usize::from(*self.key_offsets.get(index)?);
        let end = usize::from(*self.key_offsets.get(index.checked_add(1)?)?);
        self.keys.get(start..end)
    }

    /// Returns literal name `index`.
    fn literal(&self, index: usize) -> Option<&'static str> {
        let start = usize::from(*self.name_offsets.get(index)?);
        let end = usize::from(*self.name_offsets.get(index.checked_add(1)?)?);
        let name = self.names.get(start..end)?;
        Some(name)
    }

    /// Binary-searches the key arena for `needle`.
    fn find(&self, needle: &[u8]) -> Option<usize> {
        let (mut low, mut high) = (0usize, self.entries.len());
        while low < high {
            let middle = low + (high - low) / 2;
            match needle.cmp(self.key(middle)?) {
                Ordering::Less => high = middle,
                Ordering::Greater => low = middle + 1,
                Ordering::Equal => return Some(middle),
            }
        }
        None
    }

    /// Resolves `needle` (already canonicalized) by stripping its tones and synthesizing the
    /// base name with that entry's formatting rule.
    fn tone_variant<L: NameLookup>(
        &self,
        needle: &[u8],
        scratch: &mut [u8; MAX_KEY],
        lookup: &mut L,
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
        let base_name = self.materialize(index, lookup)?;
        Some(Cow::Owned(format_toned(
            code,
            &base_name,
            &tones[..tone_count],
        )))
    }

    /// Returns the 3-bit tone formatting rule of entry `index`.
    fn tone_code(&self, index: usize) -> Option<u8> {
        packed(&self.tone_codes, index, 3)
    }

    /// Returns the 2-bit published-tone-count mask of entry `index`.
    fn tone_mask(&self, index: usize) -> Option<u8> {
        packed(&self.tone_masks, index, 2)
    }

    /// Materializes the name of entry `index`.
    fn materialize<L: NameLookup>(
        &self,
        index: usize,
        lookup: &mut L,
    ) -> Option<Cow<'static, str>> {
        let packed = *self.entries.get(index)?;
        let rule = u8::try_from(packed & 0xF).ok()?;
        let argument = usize::from(packed >> 4);
        if usize::from(rule) >= RULE_COUNT {
            return None;
        }
        if rule == RULE_LITERAL {
            return Some(Cow::Borrowed(self.literal(argument)?));
        }

        let cps = Cps::parse(std::str::from_utf8(self.key(index)?).ok()?)?;
        let first = *cps.as_slice().first()?;
        match rule {
            RULE_UCD_MASK | RULE_UCD_MASK_HYPHEN => Some(Cow::Owned(ucd_mask_name(
                first,
                u8::try_from(argument).ok()?,
                rule == RULE_UCD_MASK_HYPHEN,
                lookup,
            )?)),
            RULE_KEYCAP => Some(Cow::Owned(format!("KEYCAP {}", keycap_word(first)?))),
            RULE_GENDER_WOMAN | RULE_GENDER_MAN => {
                let gender = if rule == RULE_GENDER_WOMAN {
                    "woman"
                } else {
                    "man"
                };
                let base = gender_base(cps.as_slice(), self)?;
                let base_name = self.materialize(base, lookup)?;
                Some(Cow::Owned(swap_person(&base_name, gender)))
            }
            RULE_FACING => {
                let base_cps = cps.as_slice().get(..cps.as_slice().len().checked_sub(2)?)?;
                let base = self.find_key(base_cps)?;
                let base_name = self.materialize(base, lookup)?;
                Some(Cow::Owned(format!("{base_name} facing right")))
            }
            RULE_FAMILY => Some(Cow::Owned(format!(
                "family: {}",
                join_family(cps.as_slice(), &[ZWJ])
            ))),
            RULE_COUPLE => Some(Cow::Owned(format!(
                "couple with heart: {}",
                join_family(cps.as_slice(), &[ZWJ, 0x2764, 0x1F48B])
            ))),
            RULE_KISS => Some(Cow::Owned(format!(
                "kiss: {}",
                join_family(cps.as_slice(), &[ZWJ, 0x2764, 0x1F48B])
            ))),
            RULE_PAIR => {
                let cps = cps.as_slice();
                Some(Cow::Owned(format!(
                    "{} {}",
                    plural(*cps.first()?, *cps.get(4)?)?,
                    pair_phrase(*cps.get(2)?)?
                )))
            }
            RULE_TONE_COMPONENT => {
                let tone = TONES.iter().position(|&tone| tone == first)?;
                Some(Cow::Owned(TONE_NAMES[tone].to_owned()))
            }
            RULE_CLOCK => Some(Cow::Owned(clock_name(first, lookup)?)),
            RULE_KEYCAP_10 => Some(Cow::Owned("keycap: 10".to_owned())),
            _ => None,
        }
    }

    /// Finds the entry whose canonical key is `cps`.
    fn find_key(&self, cps: &[u32]) -> Option<usize> {
        let (buffer, length) = encode_cps(cps)?;
        self.find(&buffer[..length])
    }
}

/// Returns the CLDR short name of `sequence` in `emoji`.
///
/// A free-function form of [`Emoji::emoji_name`] for callers that hold the parsed table.
#[must_use]
pub fn emoji_name<L: NameLookup>(
    emoji: &Emoji,
    lookup: &mut L,
    sequence: &str,
) -> Option<Cow<'static, str>> {
    emoji.emoji_name(lookup, sequence)
}

/// Decodes the key stream into a UTF-8 arena and its `u16` starts.
fn decode_keys(raw: &[u8], count: usize) -> Option<(Vec<u8>, Vec<u16>)> {
    let mut keys = Vec::with_capacity(raw.len().saturating_mul(2));
    let mut offsets = Vec::with_capacity(count.checked_add(1)?);
    let mut at = 0usize;
    let mut previous = 0u32;
    for _ in 0..count {
        let (delta, used) = zeta_unicode_format::read_varint(raw.get(at..)?)?;
        at += used;
        let first = previous.checked_add(delta)?;
        previous = first;
        let (length, used) = zeta_unicode_format::read_varint(raw.get(at..)?)?;
        at += used;
        let length = usize::try_from(length).ok()?;
        if length == 0 || length > MAX_CPS {
            return None;
        }
        offsets.push(u16::try_from(keys.len()).ok()?);
        push_utf8(first, &mut keys)?;
        for _ in 1..length {
            let (cp, used) = zeta_unicode_format::read_varint(raw.get(at..)?)?;
            at += used;
            push_utf8(cp, &mut keys)?;
        }
        let key_start = usize::from(*offsets.last()?);
        if keys.len().checked_sub(key_start)? > MAX_KEY {
            return None;
        }
    }
    if at != raw.len() {
        return None;
    }
    offsets.push(u16::try_from(keys.len()).ok()?);
    Some((keys, offsets))
}

/// Decodes the literal-name stream into a leaked UTF-8 arena and its `u16` starts.
fn decode_names(raw: &[u8], count: usize) -> Option<(&'static str, Vec<u16>)> {
    if count == 0 {
        return raw.is_empty().then_some(("", vec![0]));
    }
    let mut names = String::with_capacity(raw.len());
    let mut offsets = Vec::with_capacity(count.checked_add(1)?);
    let mut parts = raw.split(|&byte| byte == 0);
    for _ in 0..count {
        let part = parts.next()?;
        offsets.push(u16::try_from(names.len()).ok()?);
        names.push_str(std::str::from_utf8(part).ok()?);
    }
    if parts.next().is_some() {
        return None;
    }
    offsets.push(u16::try_from(names.len()).ok()?);
    let names: &'static str = Box::leak(names.into_boxed_str());
    Some((names, offsets))
}

/// Expands the rule and mask streams into one packed `u16` per entry.
///
/// Returns the packed entries and the number of mask bytes consumed.
fn decode_entries(rules: &[u8], masks: &[u8], literal_count: usize) -> Option<(Vec<u16>, usize)> {
    let mut entries = Vec::with_capacity(rules.len());
    let mut literal = 0usize;
    let mut mask_at = 0usize;
    for &rule in rules {
        if usize::from(rule) >= RULE_COUNT {
            return None;
        }
        let argument = match rule {
            RULE_LITERAL => {
                let ordinal = literal;
                literal = literal.checked_add(1)?;
                ordinal
            }
            RULE_UCD_MASK | RULE_UCD_MASK_HYPHEN => {
                let mask = usize::from(*masks.get(mask_at)?);
                mask_at += 1;
                mask
            }
            _ => 0,
        };
        if argument > 0xFFF {
            return None;
        }
        entries.push((u16::try_from(argument).ok()? << 4) | u16::from(rule));
    }
    if literal != literal_count {
        return None;
    }
    Some((entries, mask_at))
}

/// Appends one code point as UTF-8.
fn push_utf8(cp: u32, out: &mut Vec<u8>) -> Option<()> {
    let c = char::from_u32(cp)?;
    let mut buffer = [0u8; 4];
    out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
    Some(())
}

/// Encodes a code point slice as UTF-8 into a fixed buffer.
fn encode_cps(cps: &[u32]) -> Option<([u8; MAX_KEY], usize)> {
    let mut buffer = [0u8; MAX_KEY];
    let mut length = 0usize;
    for &cp in cps {
        let c = char::from_u32(cp)?;
        let end = length.checked_add(c.len_utf8())?;
        c.encode_utf8(buffer.get_mut(length..end)?);
        length = end;
    }
    Some((buffer, length))
}

/// Strips `U+FE0F` (`EF B8 8F`) into `buffer` without allocating.
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

/// Copies `text` into `buffer` without its skin-tone characters.
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

/// One entry's code points, on the stack.
struct Cps {
    /// The code points.
    data: [u32; MAX_CPS],
    /// How many are used.
    len: usize,
}

impl Cps {
    /// Parses a key string into its code points.
    fn parse(key: &str) -> Option<Self> {
        let mut data = [0u32; MAX_CPS];
        let mut len = 0usize;
        for c in key.chars() {
            *data.get_mut(len)? = u32::from(c);
            len += 1;
        }
        Some(Self { data, len })
    }

    /// The used code points.
    fn as_slice(&self) -> &[u32] {
        &self.data[..self.len]
    }
}

/// The first existing base a gender rule applies to.
fn gender_base(cps: &[u32], emoji: &Emoji) -> Option<usize> {
    let suffix = cps.len() >= 3
        && cps.get(cps.len() - 2) == Some(&ZWJ)
        && matches!(cps.last().copied(), Some(0x2640 | 0x2642));
    if suffix {
        if let Some(base) = emoji.find_key(cps.get(..cps.len() - 2)?) {
            return Some(base);
        }
        let mut candidate = vec![0x1F9D1];
        candidate.extend_from_slice(cps.get(1..cps.len() - 2)?);
        return emoji.find_key(&candidate);
    }
    if cps.len() == 3 && cps.get(1) == Some(&ZWJ) {
        let mut candidate = vec![0x1F9D1];
        candidate.extend_from_slice(cps.get(1..)?);
        return emoji.find_key(&candidate);
    }
    None
}

/// The `ucd_mask` materializer: selects UCD-name words, optionally hyphenating the first two.
fn ucd_mask_name<L: NameLookup>(cp: u32, mask: u8, hyphen: bool, lookup: &mut L) -> Option<String> {
    let mut name = String::new();
    if !lookup.write_name(cp, &mut name) {
        return None;
    }
    let mut out = String::new();
    let lower = name.to_ascii_lowercase();
    let mut selected = 0usize;
    for (word_index, word) in lower.split(' ').enumerate() {
        if word_index >= 8 || mask >> word_index & 1 == 0 {
            continue;
        }
        if selected > 0 {
            out.push(if hyphen && selected == 1 { '-' } else { ' ' });
        }
        out.push_str(word);
        selected += 1;
    }
    Some(out)
}

/// The clock rule: `CLOCK FACE <HOUR> OCLOCK` becomes `<hour> o’clock`.
fn clock_name<L: NameLookup>(cp: u32, lookup: &mut L) -> Option<String> {
    let mut name = String::new();
    if !lookup.write_name(cp, &mut name) {
        return None;
    }
    let mut words = name.split(' ');
    let clock = words.next()?;
    let face = words.next()?;
    let hour = words.next()?;
    let oclock = words.next()?;
    if words.next().is_some() || clock != "CLOCK" || face != "FACE" || oclock != "OCLOCK" {
        return None;
    }
    Some(format!("{} o’clock", hour.to_ascii_lowercase()))
}

/// The `swap_person` transform from the prototype.
fn swap_person(name: &str, gender: &str) -> String {
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
fn join_family(cps: &[u32], ignored: &[u32]) -> String {
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
const fn family_word(cp: u32) -> Option<&'static str> {
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
const fn plural(left: u32, right: u32) -> Option<&'static str> {
    Some(match (left, right) {
        (0x1F468, 0x1F468) => "men",
        (0x1F469, 0x1F469) => "women",
        (0x1F9D1, 0x1F9D1) => "people",
        (0x1F469, 0x1F468) => "woman and man",
        _ => return None,
    })
}

/// The phrase a pair's middle code point contributes.
const fn pair_phrase(cp: u32) -> Option<&'static str> {
    Some(match cp {
        0x1F430 => "with bunny ears",
        0x1FAEF => "wrestling",
        0x1F91D => "holding hands",
        _ => return None,
    })
}

/// The keycap word of a base code point.
const fn keycap_word(cp: u32) -> Option<&'static str> {
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

/// Reads a packed `width`-bit field that may straddle a byte boundary.
fn packed(blob: &[u8], index: usize, width: usize) -> Option<u8> {
    let bit = index.checked_mul(width)?;
    let byte = blob.get(bit / 8)?;
    let next = blob.get(bit / 8 + 1).copied().unwrap_or(0);
    let value = (u16::from(*byte) | (u16::from(next) << 8)) >> (bit % 8);
    Some(u8::try_from(value & ((1u16 << width) - 1)).expect("the field fits in a byte"))
}

/// Reads a little-endian `u16`, failing rather than wrapping.
fn u16_at(buf: &[u8], offset: usize) -> Option<u16> {
    let bytes = buf.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// Hand-written canonical Huffman decoding for the section streams.
pub mod huffman {
    /// The longest code a model may use.
    const MAX_BITS: usize = 32;

    /// Decodes a stream built by the generator.
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
            *models.get_mut(context)? = Some(Model::new(&pairs)?);
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
        fn new(pairs: &[(u8, usize)]) -> Option<Self> {
            let mut counts = [0u16; MAX_BITS + 1];
            let mut lengths = [0usize; 256];
            let mut max_bits = 0usize;
            for &(symbol, length) in pairs {
                let symbol = usize::from(symbol);
                if length == 0 || length > MAX_BITS || lengths[symbol] != 0 {
                    return None;
                }
                lengths[symbol] = length;
                counts[length] += 1;
                max_bits = max_bits.max(length);
            }
            let mut symbols = Vec::with_capacity(pairs.len());
            for length in 1..=max_bits {
                for (symbol, &symbol_length) in lengths.iter().enumerate() {
                    if symbol_length == length {
                        symbols.push(u8::try_from(symbol).ok()?);
                    }
                }
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name lookup backed by a small static map.
    struct Names;

    impl NameLookup for Names {
        fn write_name(&mut self, cp: u32, out: &mut String) -> bool {
            let name = match cp {
                0xA9 => "COPYRIGHT SIGN",
                0x1F551 => "CLOCK FACE TWO OCLOCK",
                0x1F44D => "THUMBS UP SIGN",
                _ => return false,
            };
            out.push_str(name);
            true
        }
    }

    /// A minimal flat Huffman encoder for tests: every symbol gets an eight-bit code.
    fn flat_stream(data: &[u8], order: u8) -> Vec<u8> {
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

    /// Packs `values` low `width` bits each.
    fn pack(values: &[u8], width: usize) -> Vec<u8> {
        let mut out = vec![0u8; (values.len() * width).div_ceil(8)];
        for (index, &value) in values.iter().enumerate() {
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

    /// Builds a section from explicit streams and tone arrays.
    fn section(entries: &[(Vec<u32>, u8, u16)], literals: &[&str], tones: &[(u8, u8)]) -> Vec<u8> {
        let mut key_raw = Vec::new();
        let mut previous = 0u32;
        let mut rule_raw = Vec::new();
        let mut masks = Vec::new();
        for (cps, rule, argument) in entries {
            let first = cps[0];
            zeta_unicode_format::write_varint(u64::from(first - previous), &mut key_raw);
            previous = first;
            zeta_unicode_format::write_varint(cps.len() as u64, &mut key_raw);
            for &cp in cps.iter().skip(1) {
                zeta_unicode_format::write_varint(u64::from(cp), &mut key_raw);
            }
            rule_raw.push(*rule);
            if *rule == RULE_UCD_MASK || *rule == RULE_UCD_MASK_HYPHEN {
                masks.push(u8::try_from(*argument).unwrap());
            }
        }
        let mut name_raw = Vec::new();
        for name in literals {
            name_raw.extend_from_slice(name.as_bytes());
            name_raw.push(0);
        }
        // Drop the final NUL: the decoder splits between names.
        name_raw.pop();
        let streams = [
            (flat_stream(&key_raw, 0), key_raw.len(), 0u8),
            (flat_stream(&name_raw, 0), name_raw.len(), 0),
            (flat_stream(&rule_raw, 0), rule_raw.len(), 0),
            (flat_stream(&masks, 0), masks.len(), 0),
        ];
        let mut out = Vec::new();
        out.extend_from_slice(&u16::try_from(entries.len()).unwrap().to_le_bytes());
        out.extend_from_slice(&u16::try_from(literals.len()).unwrap().to_le_bytes());
        for (stream, raw_len, order) in &streams {
            out.extend_from_slice(&u16::try_from(*raw_len).unwrap().to_le_bytes());
            out.extend_from_slice(&u16::try_from(stream.len()).unwrap().to_le_bytes());
            out.push(*order);
        }
        for (stream, _, _) in &streams {
            out.extend_from_slice(stream);
        }
        let codes: Vec<u8> = tones.iter().map(|&(code, _)| code).collect();
        let masks: Vec<u8> = tones.iter().map(|&(_, mask)| mask).collect();
        out.extend_from_slice(&pack(&codes, 3));
        out.extend_from_slice(&pack(&masks, 2));
        out
    }

    #[test]
    fn rules_materialize_without_stored_names() {
        let entries = vec![
            (vec![0x23, 0x20E3], RULE_KEYCAP, 0),
            (vec![0xA9], RULE_UCD_MASK, 3),
            (vec![0x1F44D], RULE_LITERAL, 0),
            (vec![0x1F551], RULE_CLOCK, 0),
        ];
        let blob = section(&entries, &["thumbs up"], &[(0, 0), (0, 0), (0, 0), (0, 0)]);
        let emoji = Emoji::parse(&blob).expect("the section parses");
        assert_eq!(emoji.entry_count(), 4);
        assert_eq!(
            emoji.emoji_name(&mut Names, "©").as_deref(),
            Some("copyright sign")
        );
        assert_eq!(
            emoji.emoji_name(&mut Names, "🕑").as_deref(),
            Some("two o’clock")
        );
        assert_eq!(
            emoji.emoji_name(&mut Names, "#️⃣").as_deref(),
            Some("KEYCAP NUMBER SIGN")
        );
        assert_eq!(
            emoji.emoji_name(&mut Names, "👍").as_deref(),
            Some("thumbs up")
        );
    }

    #[test]
    fn every_grammar_rule_materializes() {
        // The literal bases the rules reference, plus one entry per rule.
        let mut rows: Vec<(Vec<u32>, u8, u16, Option<&str>)> = vec![
            (vec![0x1F3C3], RULE_LITERAL, 0, Some("person running")),
            (vec![0x1F3C3, ZWJ, 0x27A1], RULE_FACING, 0, None),
            (vec![0x1F3FB], RULE_TONE_COMPONENT, 0, None),
            (
                vec![0x1F468, ZWJ, 0x1F469, ZWJ, 0x1F466],
                RULE_FAMILY,
                0,
                None,
            ),
            (
                vec![0x1F469, ZWJ, 0x2764, ZWJ, 0x1F48B, ZWJ, 0x1F468],
                RULE_KISS,
                0,
                None,
            ),
            (
                vec![0x1F469, ZWJ, 0x2764, ZWJ, 0x1F468],
                RULE_COUPLE,
                0,
                None,
            ),
            (
                vec![0x1F469, ZWJ, 0x1F91D, ZWJ, 0x1F468],
                RULE_PAIR,
                0,
                None,
            ),
            (
                vec![0x1F469, ZWJ, 0x1F9B0, ZWJ, 0x2640],
                RULE_GENDER_WOMAN,
                1,
                None,
            ),
            (vec![0x1F51F], RULE_KEYCAP_10, 0, None),
            (vec![0x1F9D1], RULE_LITERAL, 0, Some("person")),
            (
                vec![0x1F9D1, ZWJ, 0x1F9B0],
                RULE_LITERAL,
                0,
                Some("person: red hair"),
            ),
        ];
        rows.sort_by_key(|(cps, _, _, _)| {
            cps.iter()
                .flat_map(|&cp| {
                    let mut buffer = [0u8; 4];
                    char::from_u32(cp)
                        .expect("scalar value")
                        .encode_utf8(&mut buffer)
                        .as_bytes()
                        .to_vec()
                })
                .collect::<Vec<u8>>()
        });
        let entries: Vec<(Vec<u32>, u8, u16)> = rows
            .iter()
            .map(|(cps, rule, argument, _)| (cps.clone(), *rule, *argument))
            .collect();
        let literals: Vec<&str> = rows
            .iter()
            .filter_map(|(_, rule, _, name)| (*rule == RULE_LITERAL).then_some(*name).flatten())
            .collect();
        let tones = vec![(0u8, 0u8); entries.len()];
        let blob = section(&entries, &literals, &tones);
        let emoji = Emoji::parse(&blob).expect("the section parses");
        for (key, want) in [
            ("🏃‍➡", "person running facing right"),
            ("🏻", "light skin tone"),
            ("👨‍👩‍👦", "family: man, woman, boy"),
            ("👩‍❤‍💋‍👨", "kiss: woman, man"),
            ("👩‍❤‍👨", "couple with heart: woman, man"),
            ("👩‍🤝‍👨", "woman and man holding hands"),
            ("👩‍🦰‍♀", "woman: red hair"),
            ("🔟", "keycap: 10"),
        ] {
            assert_eq!(
                emoji.emoji_name(&mut Names, key).as_deref(),
                Some(want),
                "{key:?}"
            );
        }
    }

    #[test]
    fn tone_variants_use_the_published_rule_and_mask() {
        let entries = vec![(vec![0x1F44D], RULE_LITERAL, 0)];
        let blob = section(&entries, &["thumbs up"], &[(2, 1)]);
        let emoji = Emoji::parse(&blob).expect("the section parses");
        assert_eq!(
            emoji.emoji_name(&mut Names, "👍🏻").as_deref(),
            Some("thumbs up: light skin tone")
        );
        assert_eq!(emoji.emoji_name(&mut Names, "👍🏻🏿"), None);
        assert_eq!(emoji.emoji_name(&mut Names, "👍"), Some("thumbs up".into()));
        assert!(matches!(
            emoji.emoji_name(&mut Names, "👍🏻"),
            Some(Cow::Owned(_))
        ));
    }

    #[test]
    fn canonicalization_and_negative_cases_match_the_incumbent() {
        let entries = vec![(vec![0x1F44D], RULE_LITERAL, 0)];
        let blob = section(&entries, &["thumbs up"], &[(2, 1)]);
        let emoji = Emoji::parse(&blob).expect("the section parses");
        assert_eq!(emoji.emoji_name(&mut Names, ""), None);
        assert_eq!(emoji.emoji_name(&mut Names, "\u{FE0F}"), None);
        assert_eq!(emoji.emoji_name(&mut Names, "🏻👋"), None);
        assert_eq!(emoji.emoji_name(&mut Names, "\u{FE0F}👍"), None);
        assert_eq!(emoji.emoji_name(&mut Names, "d'être"), None);
        assert_eq!(emoji.emoji_name(&mut Names, "🐱‍🏍"), None);
        assert_eq!(
            emoji.emoji_name(&mut Names, "👍\u{FE0F}").as_deref(),
            Some("thumbs up")
        );
        assert!(!emoji.may_start(b'd'));
        assert!(emoji.may_start(b'\xF0'));
    }

    #[test]
    fn truncated_sections_are_rejected() {
        let entries = vec![(vec![0x1F44D], RULE_LITERAL, 0)];
        let blob = section(&entries, &["thumbs up"], &[(0, 0)]);
        assert!(Emoji::parse(&blob).is_some());
        for length in 0..blob.len() {
            assert!(Emoji::parse(&blob[..length]).is_none(), "length {length}");
        }
        assert!(Emoji::parse(&[]).is_none());
    }

    #[test]
    fn huffman_streams_round_trip_through_the_flat_test_encoder() {
        for (data, order) in [
            (b"aaaaabbbbcc".as_slice(), 0u8),
            (b"abracadabra".as_slice(), 1u8),
            (b"".as_slice(), 0u8),
            (b"x".as_slice(), 1u8),
        ] {
            let stream = flat_stream(data, order);
            assert_eq!(
                huffman::decode(&stream, data.len(), order).as_deref(),
                Some(data)
            );
        }
    }
}
