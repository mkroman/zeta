//! The emoji-section decoder shared by the runtime and the trainer.
//!
//! The section stores the 1,944 canonical entries of the shipped table in four Huffman-coded
//! streams. [`EmojiDecoder::parse`] decodes them once into a UTF-8 key arena with `u16` starts, a
//! cached first code point per entry, a UTF-8 literal-name arena, and one packed `u16` per entry
//! holding the rule id (low four bits) and its argument (high twelve bits). A lookup
//! canonicalizes `U+FE0F`, binary-searches the key arena, and materializes the name: a literal
//! is borrowed, a rule name is built. Skin-tone variants strip the tones, find the base, and
//! replay the base's 3-bit formatting rule when the 2-bit published-tone mask allows it.
//!
//! The `ucd_mask` and `clock` rules need the character name of one code point. [`NameLookup`]
//! is the seam for that: the runtime's name layer supplies it, so this module never depends on
//! the name decoder.
//!
//! Literal names are borrowed from the decoded arena; a name a rule generates is allocated on
//! every lookup, so a ruled entry returns [`Cow::Owned`] where a literal returns a borrowed
//! stored string. The text is identical; only the `Cow` variant differs.
//!
//! The trainer re-decodes the assembled section through this decoder, so verification exercises
//! the exact path the linked image uses.

use std::borrow::Cow;
use std::cmp::Ordering;

use crate::emoji::{
    MAX_CPS, MAX_KEY, Rule, RuleContext, VARIATION_SELECTOR_16, VARIATION_SELECTOR_16_UTF8,
    format_toned, is_tone, rule_name, rule_needs_sequence, tone_indices,
};
use crate::huffman;
use crate::layout::{
    EMOJI_DESCRIPTOR_LEN, EMOJI_STREAM_COUNT, EMOJI_TONE_CODE_BITS, EMOJI_TONE_MASK_BITS,
};
use crate::primitives::{encode_cps, read_varint, unpack_bits};

/// The longest chain of rule expansions a name may take.
///
/// The gender rules recurse into a base entry, but the shared renderer's `gendered_head` guard
/// bounds what the shipped table produces to a couple of levels. This cap only stops a corrupt
/// self-referential rule — a gender rule whose base resolves back to itself — before it
/// overflows the stack.
const MAX_MATERIALIZE_DEPTH: u8 = 8;

/// Supplies the Unicode character name of one code point to the emoji grammar.
///
/// The name must be the `Name` property as `DerivedName.txt` spells it (for example
/// `COPYRIGHT SIGN`), not a `NameAliases.txt` alias or a `<control-…>` label: the
/// `ucd_mask` and `clock` rules were verified against that spelling.
///
/// Implemented for any `FnMut(u32, &mut String) -> bool`; a closure can forward to a name
/// decoder without this module depending on it.
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
pub struct EmojiDecoder {
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
    /// The first code point of each entry, aligned with [`EmojiDecoder::entries`].
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

impl EmojiDecoder {
    /// Parses and decodes an emoji section.
    ///
    /// Returns `None` for a truncated or inconsistent section; every lookup on a parsed table is
    /// total.
    #[must_use]
    pub fn parse(section: &[u8]) -> Option<Self> {
        let count = usize::from(read_u16(section, 0)?);
        let literal_count = usize::from(read_u16(section, 2)?);
        if literal_count > count {
            return None;
        }
        let mut at = 4usize;
        let mut descriptors = Vec::with_capacity(EMOJI_STREAM_COUNT);
        for _ in 0..EMOJI_STREAM_COUNT {
            let raw_len = usize::from(read_u16(section, at)?);
            let coded_len = usize::from(read_u16(section, at.checked_add(2)?)?);
            let order = *section.get(at.checked_add(4)?)?;
            if order > 1 {
                return None;
            }
            descriptors.push((raw_len, coded_len, order));
            at = at.checked_add(EMOJI_DESCRIPTOR_LEN)?;
        }
        let mut streams = Vec::with_capacity(EMOJI_STREAM_COUNT);
        for &(raw_len, coded_len, order) in &descriptors {
            let coded = section.get(at..at.checked_add(coded_len)?)?;
            streams.push(huffman::decode(coded, raw_len, order)?);
            at += coded_len;
        }
        let tone_codes_len = count.checked_mul(EMOJI_TONE_CODE_BITS)?.div_ceil(8);
        let tone_masks_len = count.checked_mul(EMOJI_TONE_MASK_BITS)?.div_ceil(8);
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

        first_cps.shrink_to_fit();

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
    #[must_use]
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

    /// Returns the canonical UTF-8 key of entry `index`.
    #[must_use]
    pub fn entry_key(&self, index: usize) -> Option<&str> {
        std::str::from_utf8(self.key(index)?).ok()
    }

    /// Returns the rule and its argument for entry `index`.
    #[must_use]
    pub fn entry_rule(&self, index: usize) -> Option<(Rule, u16)> {
        Rule::unpack(*self.entries.get(index)?)
    }

    /// Materializes the name of entry `index`.
    ///
    /// A literal entry borrows from the decoded name arena; a rule entry allocates its rendered
    /// name. Returns `None` for an unknown index or a rule whose base is missing.
    #[must_use]
    pub fn entry_name<L: NameLookup>(&self, lookup: &mut L, index: usize) -> Option<Cow<'_, str>> {
        self.materialize(index, lookup, 0)
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
        if chars.next().is_some()
            && (is_tone(u32::from(first)) || u32::from(first) == VARIATION_SELECTOR_16)
        {
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
        unpack_bits(&self.tone_codes, index, EMOJI_TONE_CODE_BITS)
    }

    /// Returns the 2-bit published-tone-count mask of entry `index`.
    fn tone_mask(&self, index: usize) -> Option<u8> {
        unpack_bits(&self.tone_masks, index, EMOJI_TONE_MASK_BITS)
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
        if rule == Rule::Literal {
            return Some(Cow::Borrowed(self.literal(usize::from(argument))?));
        }
        let cps = if rule_needs_sequence(rule) {
            self.entry_cps(index)?
        } else {
            Cps::single(self.first_cp(index)?)
        };
        let mut context = MaterializeContext {
            emoji: self,
            lookup,
            depth,
        };
        Some(Cow::Owned(rule_name(
            rule,
            argument,
            cps.as_slice(),
            &mut context,
        )?))
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

/// The decoder's table lookups for the shared emoji rule renderer.
struct MaterializeContext<'a, 'b, L: NameLookup> {
    /// The decoded section the base keys resolve against.
    emoji: &'a EmojiDecoder,
    /// The character-name lookup for `ucd_mask` and `clock`.
    lookup: &'b mut L,
    /// The expansions already taken, so a recursive base stops at the shared depth cap.
    depth: u8,
}

impl<L: NameLookup> RuleContext for MaterializeContext<'_, '_, L> {
    fn ucd_name(&mut self, cp: u32) -> Option<String> {
        let mut name = String::new();
        self.lookup.write_name(cp, &mut name).then_some(name)
    }

    fn base_name(&mut self, cps: &[u32]) -> Option<String> {
        let index = self.emoji.find_key(cps)?;
        self.emoji
            .materialize(index, self.lookup, self.depth + 1)
            .map(Cow::into_owned)
    }
}

/// Reads a little-endian `u16` at `at`.
fn read_u16(section: &[u8], at: usize) -> Option<u16> {
    let bytes: [u8; 2] = section.get(at..at.checked_add(2)?)?.try_into().ok()?;
    Some(u16::from_le_bytes(bytes))
}

/// Decodes the key stream into a UTF-8 arena and its `u16` starts.
fn decode_keys(raw: &[u8], count: usize) -> Option<(Vec<u8>, Vec<u16>)> {
    let mut keys = Vec::with_capacity(raw.len().saturating_mul(2));
    let mut offsets = Vec::with_capacity(count.checked_add(1)?);
    let mut at = 0usize;
    let mut previous = 0u32;
    for _ in 0..count {
        let (delta, used) = read_varint(raw.get(at..)?)?;
        at += used;
        let first = previous.checked_add(delta)?;
        previous = first;
        let (length, used) = read_varint(raw.get(at..)?)?;
        at += used;
        let length = usize::try_from(length).ok()?;
        if length == 0 || length > MAX_CPS {
            return None;
        }
        offsets.push(u16::try_from(keys.len()).ok()?);
        push_utf8(first, &mut keys)?;
        for _ in 1..length {
            let (cp, used) = read_varint(raw.get(at..)?)?;
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
    keys.shrink_to_fit();
    offsets.shrink_to_fit();
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
    offsets.shrink_to_fit();
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

/// Strips the UTF-8 bytes of [`VARIATION_SELECTOR_16`] into `buffer` without allocating.
fn canonical_key<'a>(key: &[u8], buffer: &'a mut [u8; MAX_KEY]) -> Option<&'a [u8]> {
    let marker = VARIATION_SELECTOR_16_UTF8;
    let mut used = 0usize;
    let mut index = 0usize;
    while index < key.len() {
        if key[index..].starts_with(&marker) {
            index += marker.len();
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
    /// A one-code-point sequence for the rules that only need the first code point.
    const fn single(cp: u32) -> Self {
        let mut data = [0u32; MAX_CPS];
        data[0] = cp;
        Self { data, len: 1 }
    }

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
