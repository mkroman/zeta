//! Nomina emoji lookup: canonical keys, binary search and rule-generated names.
//!
//! The section stores the 1,944 canonical entries of the shipped table in four Huffman-coded
//! streams. [`Emoji::parse`] decodes them once into a UTF-8 key arena with `u16` starts, a
//! cached first code point per entry, a UTF-8 literal-name arena, and one packed `u16` per entry
//! holding the rule id (low four bits) and its argument (high twelve bits). A lookup
//! canonicalizes `U+FE0F`, binary-searches the key arena, and materializes the name: a literal
//! is borrowed, a rule name is built. Skin-tone variants strip the tones, find the base, and
//! replay the base's 3-bit formatting rule when the 2-bit published-tone mask allows it.
//!
//! The `ucd_mask` and `clock` rules need the character name of one code point. [`NameLookup`]
//! is the seam for that: the name layer supplies it, so this module never depends on the name
//! decoder.
//!
//! Literal names are borrowed from the decoded arena; a name a rule generates is allocated on
//! every lookup, so a ruled entry returns [`Cow::Owned`] where a literal returns a borrowed
//! stored string. The text is identical; only the `Cow` variant differs.

use std::borrow::Cow;
use std::cmp::Ordering;

use bytes::Buf;
use zeta_unicode_format::huffman;
use zeta_unicode_format::{
    MAX_CPS, MAX_KEY, Rule, TONE_NAMES, TONES, ZWJ, clock_name, encode_cps, format_toned, is_tone,
    join_family, keycap_word, pair_phrase, plural, swap_person, tone_indices, ucd_mask_name,
    unpack_bits,
};

/// Four stream descriptors of `u16 raw_len | u16 coded_len | u8 order`.
const STREAM_COUNT: usize = 4;

/// The longest chain of rule expansions a name may take.
///
/// The gender rules recurse into a base entry, but the [`gendered_head`] guard bounds what
/// the shipped table produces to a couple of levels. This cap only stops a corrupt
/// self-referential rule — a gender rule whose base resolves back to itself — before it
/// overflows the stack.
const MAX_MATERIALIZE_DEPTH: u8 = 8;

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
    names: String,
    /// Start offset of each literal name, with the arena length appended.
    name_offsets: Vec<u16>,
    /// One packed `(argument << 4) | rule id` per entry.
    entries: Vec<u16>,
    /// The first code point of each entry, aligned with [`Emoji::entries`].
    ///
    /// Four bytes per entry, about 7.8 KB for the shipped table. It saves decoding the whole
    /// key for the rules that only look at the first code point: `ucd_mask`, keycap,
    /// tone-component and clock.
    first_cps: Vec<u32>,
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
    /// total.
    #[must_use]
    pub fn parse(section: &[u8]) -> Option<Self> {
        let mut header = section;
        let count = usize::from(header.try_get_u16_le().ok()?);
        let literal_count = usize::from(header.try_get_u16_le().ok()?);
        if literal_count > count {
            return None;
        }
        let mut descriptors = Vec::with_capacity(STREAM_COUNT);
        for _ in 0..STREAM_COUNT {
            let raw_len = usize::from(header.try_get_u16_le().ok()?);
            let coded_len = usize::from(header.try_get_u16_le().ok()?);
            let order = header.try_get_u8().ok()?;
            if order > 1 {
                return None;
            }
            descriptors.push((raw_len, coded_len, order));
        }
        let mut at = section.len().checked_sub(header.remaining())?;
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
        let mut first_cps = Vec::with_capacity(key_offsets.len().checked_sub(1)?);
        let mut previous: Option<&[u8]> = None;
        for pair in key_offsets.windows(2) {
            let start = usize::from(pair[0]);
            let end = usize::from(pair[1]);
            let key = keys.get(start..end)?;
            if previous.is_some_and(|previous| previous >= key) {
                return None;
            }
            previous = Some(key);
            let first = *key.first()?;
            first_bytes[usize::from(first) >> 6] |= 1 << (first & 63);
            // The arena is built from validated scalars, so this cannot fail; decoding the
            // first character here keeps `materialize` from walking the key again.
            let text = std::str::from_utf8(key).ok()?;
            first_cps.push(u32::from(text.chars().next()?));
        }

        Some(Self {
            keys,
            key_offsets,
            names,
            name_offsets,
            entries,
            first_cps,
            tone_codes,
            tone_masks,
            first_bytes,
        })
    }

    /// The number of stored canonical entries.
    #[cfg(test)]
    pub const fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether a stored key can start with `byte`.
    ///
    /// This is the admission check, not a match.
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
    ) -> Option<Cow<'_, str>> {
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
            return self.materialize(index, lookup, 0);
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
    fn literal(&self, index: usize) -> Option<&str> {
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
    ) -> Option<Cow<'_, str>> {
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
        let base_name = self.materialize(index, lookup, 0)?;
        Some(Cow::Owned(format_toned(
            code,
            &base_name,
            &tones[..tone_count],
        )))
    }

    /// Returns the 3-bit tone formatting rule of entry `index`.
    fn tone_code(&self, index: usize) -> Option<u8> {
        unpack_bits(&self.tone_codes, index, 3)
    }

    /// Returns the 2-bit published-tone-count mask of entry `index`.
    fn tone_mask(&self, index: usize) -> Option<u8> {
        unpack_bits(&self.tone_masks, index, 2)
    }

    /// Materializes the name of entry `index`.
    ///
    /// Rules that only need the first code point read it from the parse-time cache; only the
    /// rules that inspect the whole sequence decode it into [`Cps`]. `depth` counts the rule
    /// expansions already taken: a corrupt rule whose base resolves back to itself stops at
    /// [`MAX_MATERIALIZE_DEPTH`] with `None` instead of overflowing the stack.
    fn materialize<L: NameLookup>(
        &self,
        index: usize,
        lookup: &mut L,
        depth: u8,
    ) -> Option<Cow<'_, str>> {
        if depth >= MAX_MATERIALIZE_DEPTH {
            return None;
        }
        let packed = *self.entries.get(index)?;
        let (rule, argument) = Rule::unpack(packed)?;
        let argument = usize::from(argument);
        match rule {
            Rule::Literal => Some(Cow::Borrowed(self.literal(argument)?)),
            Rule::UcdMask | Rule::UcdMaskHyphen => {
                let mask = u8::try_from(argument).ok()?;
                Some(Cow::Owned(lookup_name(
                    self.first_cp(index)?,
                    lookup,
                    |name| ucd_mask_name(name, mask, rule == Rule::UcdMaskHyphen),
                )?))
            }
            Rule::Keycap => Some(Cow::Owned(format!(
                "KEYCAP {}",
                keycap_word(self.first_cp(index)?)?
            ))),
            Rule::GenderWoman | Rule::GenderMan => {
                let gender = if rule == Rule::GenderWoman {
                    "woman"
                } else {
                    "man"
                };
                let base = gender_base(self.entry_cps(index)?.as_slice(), self)?;
                let base_name = self.materialize(base, lookup, depth + 1)?;
                Some(Cow::Owned(swap_person(&base_name, gender)))
            }
            Rule::Facing => {
                let cps = self.entry_cps(index)?;
                let cps = cps.as_slice();
                let base = self.find_key(cps.get(..cps.len().checked_sub(2)?)?)?;
                let base_name = self.materialize(base, lookup, depth + 1)?;
                Some(Cow::Owned(format!("{base_name} facing right")))
            }
            Rule::Family => Some(Cow::Owned(format!(
                "family: {}",
                join_family(self.entry_cps(index)?.as_slice(), &[ZWJ])
            ))),
            Rule::Couple => Some(Cow::Owned(format!(
                "couple with heart: {}",
                join_family(self.entry_cps(index)?.as_slice(), &[ZWJ, 0x2764, 0x1F48B])
            ))),
            Rule::Kiss => Some(Cow::Owned(format!(
                "kiss: {}",
                join_family(self.entry_cps(index)?.as_slice(), &[ZWJ, 0x2764, 0x1F48B])
            ))),
            Rule::Pair => {
                let cps = self.entry_cps(index)?;
                let cps = cps.as_slice();
                Some(Cow::Owned(format!(
                    "{} {}",
                    plural(*cps.first()?, *cps.get(4)?)?,
                    pair_phrase(*cps.get(2)?)?
                )))
            }
            Rule::ToneComponent => {
                let first = self.first_cp(index)?;
                let tone = TONES.iter().position(|&tone| tone == first)?;
                Some(Cow::Borrowed(TONE_NAMES[tone]))
            }
            Rule::Clock => lookup_name(self.first_cp(index)?, lookup, clock_name)
                .flatten()
                .map(Cow::Owned),
            Rule::Keycap10 => Some(Cow::Borrowed("keycap: 10")),
        }
    }

    /// Returns the cached first code point of entry `index`.
    fn first_cp(&self, index: usize) -> Option<u32> {
        self.first_cps.get(index).copied()
    }

    /// Decodes the full key of entry `index` into its code points.
    fn entry_cps(&self, index: usize) -> Option<Cps> {
        Cps::parse(std::str::from_utf8(self.key(index)?).ok()?)
    }

    /// Finds the entry whose canonical key is `cps`.
    fn find_key(&self, cps: &[u32]) -> Option<usize> {
        let mut buffer = [0u8; MAX_KEY];
        let length = encode_cps(cps, &mut buffer)?;
        self.find(&buffer[..length])
    }
}

/// Resolves a rule that needs the UCD character name through a name lookup.
fn lookup_name<L, R>(cp: u32, lookup: &mut L, render: impl FnOnce(&str) -> R) -> Option<R>
where
    L: NameLookup,
{
    let mut name = String::new();
    lookup.write_name(cp, &mut name).then(|| render(&name))
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

/// Decodes the literal-name stream into a UTF-8 arena and its `u16` starts.
fn decode_names(raw: &[u8], count: usize) -> Option<(String, Vec<u16>)> {
    if count == 0 {
        return raw.is_empty().then_some((String::new(), vec![0]));
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
    Some((names, offsets))
}

/// Expands the rule and mask streams into one packed `u16` per entry.
///
/// Returns the packed entries and the number of mask bytes consumed.
fn decode_entries(rules: &[u8], masks: &[u8], literal_count: usize) -> Option<(Vec<u16>, usize)> {
    let mut entries = Vec::with_capacity(rules.len());
    let mut literal = 0usize;
    let mut mask_at = 0usize;
    for &id in rules {
        let rule = Rule::from_id(id)?;
        let argument = match rule {
            Rule::Literal => {
                let ordinal = literal;
                literal = literal.checked_add(1)?;
                ordinal
            }
            Rule::UcdMask | Rule::UcdMaskHyphen => {
                let mask = usize::from(*masks.get(mask_at)?);
                mask_at += 1;
                mask
            }
            Rule::Keycap
            | Rule::GenderWoman
            | Rule::GenderMan
            | Rule::Facing
            | Rule::Family
            | Rule::Couple
            | Rule::Kiss
            | Rule::Pair
            | Rule::ToneComponent
            | Rule::Clock
            | Rule::Keycap10 => 0,
        };
        entries.push(rule.pack(u16::try_from(argument).ok()?)?);
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

/// The head code points a gender rule may rewrite to the `person` base.
///
/// The trainer only assigns a gender rule when its own rewrite check passed for a man or
/// woman head (`gender_rule`/`role_rule` reject every other head), so the runtime mirrors
/// that guard: a corrupt table then answers `None` instead of synthesizing a base the
/// trainer never verified.
const fn gendered_head(cp: u32) -> bool {
    matches!(cp, 0x1F468 | 0x1F469)
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
        if !cps.first().is_some_and(|&head| gendered_head(head)) {
            return None;
        }
        let mut candidate = vec![0x1F9D1];
        candidate.extend_from_slice(cps.get(1..cps.len() - 2)?);
        return emoji.find_key(&candidate);
    }
    if cps.len() == 3
        && cps.get(1) == Some(&ZWJ)
        && cps.first().is_some_and(|&head| gendered_head(head))
    {
        let mut candidate = vec![0x1F9D1];
        candidate.extend_from_slice(cps.get(1..)?);
        return emoji.find_key(&candidate);
    }
    None
}

#[cfg(test)]
mod tests {
    use zeta_unicode_format::pack_bits;

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

    /// Builds a section from explicit streams and tone arrays.
    fn section(
        entries: &[(Vec<u32>, Rule, u16)],
        literals: &[&str],
        tones: &[(u8, u8)],
    ) -> Vec<u8> {
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
            rule_raw.push(rule.id());
            if matches!(*rule, Rule::UcdMask | Rule::UcdMaskHyphen) {
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
            (huffman::encode(&key_raw, 0), key_raw.len(), 0u8),
            (huffman::encode(&name_raw, 0), name_raw.len(), 0),
            (huffman::encode(&rule_raw, 0), rule_raw.len(), 0),
            (huffman::encode(&masks, 0), masks.len(), 0),
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
        out.extend_from_slice(&pack_bits(codes, 3));
        out.extend_from_slice(&pack_bits(masks, 2));
        out
    }

    #[test]
    fn rules_materialize_without_stored_names() {
        let entries = vec![
            (vec![0x23, 0x20E3], Rule::Keycap, 0),
            (vec![0xA9], Rule::UcdMask, 3),
            (vec![0x1F44D], Rule::Literal, 0),
            (vec![0x1F551], Rule::Clock, 0),
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
        let mut rows: Vec<(Vec<u32>, Rule, u16, Option<&str>)> = vec![
            (vec![0x1F3C3], Rule::Literal, 0, Some("person running")),
            (vec![0x1F3C3, ZWJ, 0x27A1], Rule::Facing, 0, None),
            (vec![0x1F3FB], Rule::ToneComponent, 0, None),
            (
                vec![0x1F468, ZWJ, 0x1F469, ZWJ, 0x1F466],
                Rule::Family,
                0,
                None,
            ),
            (
                vec![0x1F469, ZWJ, 0x2764, ZWJ, 0x1F48B, ZWJ, 0x1F468],
                Rule::Kiss,
                0,
                None,
            ),
            (
                vec![0x1F469, ZWJ, 0x2764, ZWJ, 0x1F468],
                Rule::Couple,
                0,
                None,
            ),
            (
                vec![0x1F469, ZWJ, 0x1F91D, ZWJ, 0x1F468],
                Rule::Pair,
                0,
                None,
            ),
            (
                vec![0x1F469, ZWJ, 0x1F9B0, ZWJ, 0x2640],
                Rule::GenderWoman,
                1,
                None,
            ),
            (vec![0x1F51F], Rule::Keycap10, 0, None),
            (vec![0x1F9D1], Rule::Literal, 0, Some("person")),
            (
                vec![0x1F9D1, ZWJ, 0x1F9B0],
                Rule::Literal,
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
        let entries: Vec<(Vec<u32>, Rule, u16)> = rows
            .iter()
            .map(|(cps, rule, argument, _)| (cps.clone(), *rule, *argument))
            .collect();
        let literals: Vec<&str> = rows
            .iter()
            .filter_map(|(_, rule, _, name)| (*rule == Rule::Literal).then_some(*name).flatten())
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
    fn an_adult_headed_gender_rule_does_not_rewrite_its_base() {
        // The trainer never assigns a gender rule to a `person` head, so the runtime's head
        // guard answers `None` for this corrupt entry instead of rewriting it to itself.
        let entries = vec![(vec![0x1F9D1, ZWJ, 0x1F9B0], Rule::GenderWoman, 0)];
        let blob = section(&entries, &[], &[(0, 0)]);
        let emoji = Emoji::parse(&blob).expect("the section parses");
        assert_eq!(emoji.emoji_name(&mut Names, "🧑‍🦰"), None);
    }

    #[test]
    fn a_gender_rule_rewrites_a_man_or_woman_head_to_the_person_base() {
        // Both roles resolve through the person base one level down; an adult head carrying
        // the rule instead is rejected by the head guard
        // (`an_adult_headed_gender_rule_does_not_rewrite_its_base`).
        let entries = vec![
            (vec![0x1F468, ZWJ, 0x1F9B0], Rule::GenderMan, 0),
            (vec![0x1F469, ZWJ, 0x1F9B0], Rule::GenderWoman, 0),
            (vec![0x1F9D1, ZWJ, 0x1F9B0], Rule::Literal, 0),
        ];
        let blob = section(&entries, &["person: red hair"], &[(0, 0), (0, 0), (0, 0)]);
        let emoji = Emoji::parse(&blob).expect("the section parses");
        assert_eq!(
            emoji.emoji_name(&mut Names, "👨‍🦰").as_deref(),
            Some("man: red hair")
        );
        assert_eq!(
            emoji.emoji_name(&mut Names, "👩‍🦰").as_deref(),
            Some("woman: red hair")
        );
    }

    #[test]
    fn tone_variants_use_the_published_rule_and_mask() {
        let entries = vec![(vec![0x1F44D], Rule::Literal, 0)];
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
        let entries = vec![(vec![0x1F44D], Rule::Literal, 0)];
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
        let entries = vec![(vec![0x1F44D], Rule::Literal, 0)];
        let blob = section(&entries, &["thumbs up"], &[(0, 0)]);
        assert!(Emoji::parse(&blob).is_some());
        for length in 0..blob.len() {
            assert!(Emoji::parse(&blob[..length]).is_none(), "length {length}");
        }
        assert!(Emoji::parse(&[]).is_none());
    }
}
