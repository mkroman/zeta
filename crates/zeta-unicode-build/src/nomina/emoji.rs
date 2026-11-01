//! Nomina emoji section: rule-generated names over Huffman-coded streams.
//!
//! The section stores the 1,944 canonical entries of the shipped table, but most names are not
//! stored: 1,341 entries name themselves through one of the thirteen grammar rules measured in
//! the prototype (`ucd_mask`, gender swaps, family, facing, keycap, ...). Every rule candidate is
//! replayed here against the published name, and any mismatch falls back to a literal, so the
//! encoder can never publish a name it did not reproduce.
//!
//! Streams are hand-written canonical Huffman over an order-0 or order-1 byte model. Keys are
//! sorted canonical UTF-8 sequences, delta-coded by first code point; names are the literal
//! names in entry order; rules are one `u8` per entry with an optional argument byte; skin-tone
//! rules and published-tone masks are bit-packed. The runtime decodes the four streams once and
//! binary-searches a UTF-8 key arena.
//!
//! # Layout
//!
//! ```text
//! u16 entries | u16 literal_count
//! four descriptors: u16 raw_len | u16 coded_len | u8 order
//! keys   (order 0 or 1) | names | rules | masks
//! packed 3-bit tone codes | packed 2-bit tone masks
//! ```
//!
//! # Panics
//!
//! Malformed vendored data is an invariant violation and panics with a message naming the
//! offending entry. This is build-time code generation: a bad byte is a build failure.

use std::collections::BTreeMap;

use bytes::BufMut;

use zeta_unicode_format::{
    MAX_CPS, MAX_KEY, MAX_RULE_ARGUMENT, Rule, RuleContext, TONE_NAMES, TONES, ZWJ, clock_name,
    encode_cps, family_word, format_toned, is_tone, join_family, keycap_word, pack_bits,
    pair_phrase, plural, rule_name, swap_person, tone_phrase, tone_positions, ucd_mask_name,
    write_varint,
};

use zeta_unicode_format::huffman;

use crate::ucd::{hex_u32, records};

/// One canonical emoji entry, as it should appear in the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmojiEntry {
    /// Canonical UTF-8 key with `U+FE0F` removed.
    pub key: String,
    /// The published CLDR name.
    pub name: String,
    /// The 3-bit tone formatting rule (`0` when the base has no variants).
    pub tone_code: u8,
    /// The 2-bit published-tone-count mask.
    pub tone_mask: u8,
}

/// One skin-tone variant: its tone indices, its name and its canonical code points.
type ToneVariant = (Vec<usize>, String, Vec<u32>);

/// Tone variants grouped by the sequence they derive from (with the tones removed).
type ToneVariants = BTreeMap<Vec<u32>, Vec<ToneVariant>>;

/// Counters and measured sizes for one emoji section build.
#[derive(Clone, Debug, Default)]
pub struct EmojiStats {
    /// Canonical entries encoded.
    pub entries: usize,
    /// Entries stored as literal names.
    pub literals: usize,
    /// Entries whose name a grammar rule reproduces.
    pub ruled: usize,
    /// Entries per grammar rule, indexed by [`Rule::id`].
    pub rule_counts: [usize; Rule::COUNT],
    /// Total encoded section size, in bytes.
    pub section_bytes: usize,
    /// Encoded key stream size.
    pub key_stream: usize,
    /// Encoded literal-name stream size.
    pub name_stream: usize,
    /// Encoded rule stream size.
    pub rule_stream: usize,
    /// Encoded mask stream size.
    pub mask_stream: usize,
    /// Candidates that a rule could not reproduce and that fell back to a literal.
    pub fallbacks: usize,
    /// Packed tone-code and tone-mask bytes.
    pub tone_bytes: usize,
    /// Raw (pre-Huffman) key stream size.
    pub key_raw: usize,
    /// Raw literal-name stream size.
    pub name_raw: usize,
    /// Raw rule stream size.
    pub rule_raw: usize,
    /// Raw mask stream size.
    pub mask_raw: usize,
    /// Decoded runtime heap: key arena, offsets, names, entries, first code points, the
    /// first-byte bit set and tone bytes.
    pub heap_bytes: usize,
}

/// Builds the emoji section from the vendored `emoji-test.txt`, `NamedSequences.txt` and
/// `DerivedName.txt` texts.
///
/// Every grammar rule is replayed against the published name before it is accepted; a rule that
/// does not reproduce the name falls back to a literal. Every stream round-trips through the
/// decoder before the section is returned.
#[must_use]
pub fn build(emoji_text: &str, named_text: &str, derived_names: &str) -> (Vec<u8>, EmojiStats) {
    let entries = canonical_entries(emoji_text, named_text);
    let ucd = ucd_names(derived_names);
    emoji_section(&entries, &ucd)
}

/// Parses the canonical emoji entries exactly as the shipped table does.
///
/// `NamedSequences.txt` names win over fully-qualified entries, which win over minimally
/// qualified and unqualified ones. Keys are canonicalized by removing `U+FE0F`; tone variants
/// are folded into their base, and a base records the 3-bit formatting rule and 2-bit published
/// tone counts. Variants that no rule reproduces are stored verbatim.
///
/// # Panics
///
/// Panics when a vendored line is malformed or a canonical key is not a scalar value. That is
/// a build failure, not a runtime condition.
#[must_use]
pub fn canonical_entries(emoji_text: &str, named_text: &str) -> Vec<EmojiEntry> {
    let all = parse_emoji(emoji_text, named_text);

    // Canonicalize: strip U+FE0F, keeping the highest-precedence name per canonical key.
    let mut canonical: BTreeMap<Vec<u32>, (u8, String)> = BTreeMap::new();
    for (cps, (rank, name)) in &all {
        keep_best(&mut canonical, canonical_cps(cps), *rank, name.clone());
    }

    let mut kept: BTreeMap<Vec<u32>, (String, u8, u8)> = BTreeMap::new();
    let mut variants: ToneVariants = BTreeMap::new();
    for (cps, (_rank, name)) in &canonical {
        let tones = tone_positions(cps.iter().copied());
        if tones.is_empty() || cps.len() == 1 {
            kept.insert(cps.clone(), (name.clone(), 0, 0));
        } else {
            let base: Vec<u32> = cps.iter().copied().filter(|cp| !is_tone(*cp)).collect();
            variants
                .entry(base)
                .or_default()
                .push((tones, name.clone(), cps.clone()));
        }
    }

    for (base, group) in &variants {
        if group
            .iter()
            .any(|(tones, _, _)| !(1..=2).contains(&tones.len()))
        {
            store_verbatim(&mut kept, group);
            continue;
        }

        let (base_name, mut rule) = match kept.get(base) {
            Some((name, _, _)) => (name.clone(), 0),
            None if let Some((name, rule)) = derive_base(group) => {
                kept.insert(base.clone(), (name.clone(), rule, 0));
                (name, rule)
            }
            None => {
                store_verbatim(&mut kept, group);
                continue;
            }
        };

        if rule == 0 {
            for candidate in [3u8, 1, 4, 2] {
                if group.iter().all(|(tones, variant_name, _)| {
                    format_toned(candidate, &base_name, tones) == *variant_name
                }) {
                    rule = candidate;
                    break;
                }
            }
        }

        let tone_mask = group.iter().fold(0u8, |mask, (tones, _, _)| {
            mask | match tones.len() {
                1 => 1,
                2 => 2,
                _ => 0,
            }
        });
        if rule == 0 {
            store_verbatim(&mut kept, group);
            continue;
        }

        {
            let entry = kept.get_mut(base).expect("base entry exists");
            entry.1 = rule;
            entry.2 = tone_mask;
        }
    }

    let mut entries: Vec<EmojiEntry> = kept
        .into_iter()
        .map(|(cps, (name, tone_code, tone_mask))| {
            let key: String = cps
                .iter()
                .map(|&cp| char::from_u32(cp).expect("code point is a scalar value"))
                .collect();
            EmojiEntry {
                key,
                name,
                tone_code,
                tone_mask,
            }
        })
        .collect();
    entries.sort_by(|left, right| left.key.as_bytes().cmp(right.key.as_bytes()));
    for pair in entries.windows(2) {
        assert!(
            pair[0].key < pair[1].key,
            "duplicate emoji key {:?}",
            pair[0].key
        );
    }
    entries
}

/// Parses the single-code-point names of `DerivedName.txt` into a code point map.
///
/// Range lines and the `*` patterns are skipped: the grammar only asks for the name of one
/// code point.
///
/// # Panics
///
/// Panics when a vendored line is malformed. That is a build failure, not a runtime condition.
#[must_use]
pub fn ucd_names(derived_names: &str) -> BTreeMap<u32, String> {
    let mut names = BTreeMap::new();
    for (number, line) in records(derived_names) {
        let (lhs, rhs) = line
            .split_once(';')
            .unwrap_or_else(|| panic!("line {number}: {line}"));
        let lhs = lhs.trim();
        let rhs = rhs.trim();
        if rhs.contains('*') || lhs.contains("..") {
            continue;
        }
        names.insert(hex_u32(lhs), rhs.to_owned());
    }
    names
}

/// Encodes the emoji section for `entries`, using `ucd` for the `ucd_mask` and `clock` rules.
///
/// # Panics
///
/// Panics when an entry is malformed (an empty key, more code points than the runtime decodes,
/// or a key over `MAX_KEY`), when a stream or arena outgrows its `u16` length or the 12-bit
/// rule argument, or when the assembled section does not round-trip. That is a build failure,
/// not a runtime condition.
#[must_use]
pub fn emoji_section(entries: &[EmojiEntry], ucd: &BTreeMap<u32, String>) -> (Vec<u8>, EmojiStats) {
    let all_cps: Vec<Vec<u32>> = entries
        .iter()
        .map(|entry| code_points(&entry.key))
        .collect();
    let mut stats = EmojiStats {
        entries: entries.len(),
        ..EmojiStats::default()
    };
    let (assigned, literal_names, masks) = assign_rules(entries, &all_cps, ucd, &mut stats);
    let payload = encode_payload(entries, &all_cps, &assigned, &literal_names, &masks);
    verify_streams(
        entries,
        &all_cps,
        &assigned,
        &payload.decoded_keys,
        &payload.decoded_names,
        ucd,
    );
    let section = assemble(entries.len(), literal_names.len(), &payload);

    stats.literals = literal_names.len();
    stats.ruled = entries.len() - stats.literals;
    stats.section_bytes = section.len();
    stats.key_stream = payload.key_stream.len();
    stats.name_stream = payload.name_stream.len();
    stats.rule_stream = payload.rule_stream.len();
    stats.mask_stream = payload.mask_stream.len();
    stats.tone_bytes = payload.tone_codes.len() + payload.tone_masks.len();
    stats.key_raw = payload.key_raw.len();
    stats.name_raw = payload.name_raw.len();
    stats.rule_raw = payload.rule_raw.len();
    stats.mask_raw = masks.len();
    stats.heap_bytes = payload.key_bytes
        + (entries.len() + 1) * 2
        + payload.literal_bytes
        + (literal_names.len() + 1) * 2
        + entries.len() * 2
        // The runtime also keeps one u32 first code point per entry and a [u64; 4]
        // first-byte admission set; both are part of the decoded footprint.
        + entries.len() * 4
        + 32
        + stats.tone_bytes;
    (section, stats)
}

/// Assigns one rule per entry, verifying it against the published name and falling back to a
/// literal.
fn assign_rules(
    entries: &[EmojiEntry],
    all_cps: &[Vec<u32>],
    ucd: &BTreeMap<u32, String>,
    stats: &mut EmojiStats,
) -> (Vec<(Rule, u16)>, Vec<String>, Vec<u8>) {
    let by_key: BTreeMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.key.as_str(), index))
        .collect();
    assert_eq!(by_key.len(), entries.len(), "duplicate emoji keys");

    let mut assigned = Vec::with_capacity(entries.len());
    let mut literal_names = Vec::new();
    let mut masks = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let cps = &all_cps[index];
        let candidate = rule_for(entry, cps, entries, ucd);
        let accepted = candidate.filter(|&(rule, arg)| {
            generated_name(rule, arg, cps, entries, ucd).as_deref() == Some(entry.name.as_str())
        });
        let (rule, arg) = if let Some((rule, arg)) = accepted {
            (rule, arg)
        } else {
            if candidate.is_some() {
                stats.fallbacks += 1;
            }
            let ordinal = u16::try_from(literal_names.len()).expect("fewer than 65536 literals");
            literal_names.push(entry.name.clone());
            (Rule::Literal, ordinal)
        };
        stats.rule_counts[usize::from(rule.id())] += 1;
        if matches!(rule, Rule::UcdMask | Rule::UcdMaskHyphen) {
            masks.push(u8::try_from(arg).expect("a mask fits in a byte"));
        }
        assigned.push((rule, arg));
    }
    assert_eq!(
        stats.rule_counts[usize::from(Rule::Literal.id())],
        literal_names.len(),
        "literal rule count"
    );
    (assigned, literal_names, masks)
}

/// The raw streams, their coded forms, the packed tone arrays and the round-trip decodes.
struct Payload {
    key_raw: Vec<u8>,
    name_raw: Vec<u8>,
    rule_raw: Vec<u8>,
    mask_raw: Vec<u8>,
    key_stream: Vec<u8>,
    name_stream: Vec<u8>,
    rule_stream: Vec<u8>,
    mask_stream: Vec<u8>,
    decoded_keys: Vec<u8>,
    decoded_names: Vec<u8>,
    tone_codes: Vec<u8>,
    tone_masks: Vec<u8>,
    key_bytes: usize,
    literal_bytes: usize,
}

/// Encodes the key, literal-name, rule and mask streams and round-trips each one.
fn encode_payload(
    entries: &[EmojiEntry],
    all_cps: &[Vec<u32>],
    assigned: &[(Rule, u16)],
    literal_names: &[String],
    masks: &[u8],
) -> Payload {
    // Keys: sorted canonical UTF-8 sequences, delta-coded by first code point.
    let mut key_raw = Vec::new();
    let mut previous = 0u32;
    let mut key_bytes = 0usize;
    for (entry, cps) in entries.iter().zip(all_cps) {
        assert!(!cps.is_empty(), "empty emoji key");
        assert!(
            entry.key.len() <= MAX_KEY,
            "emoji key {:?} is over MAX_KEY",
            entry.key
        );
        assert!(
            cps.len() <= MAX_CPS,
            "emoji key {:?} has {} code points, over the {MAX_CPS} the runtime decodes",
            entry.key,
            cps.len()
        );
        let first = *cps.first().expect("a non-empty key");
        assert!(first >= previous, "keys are not sorted by first code point");
        write_varint(first - previous, &mut key_raw);
        previous = first;
        write_varint(
            u32::try_from(cps.len()).expect("a key length"),
            &mut key_raw,
        );
        for &cp in cps.iter().skip(1) {
            write_varint(cp, &mut key_raw);
        }
        key_bytes += entry.key.len();
    }

    // Literal names in entry order, NUL-separated.
    let mut name_raw = Vec::new();
    let mut literal_bytes = 0usize;
    for (index, name) in literal_names.iter().enumerate() {
        if index > 0 {
            name_raw.push(0);
        }
        literal_bytes += name.len();
        name_raw.extend_from_slice(name.as_bytes());
    }

    let rule_raw: Vec<u8> = assigned.iter().map(|&(rule, _)| rule.id()).collect();
    let key_stream = huffman::encode(&key_raw, 1);
    let name_stream = huffman::encode(&name_raw, 0);
    let rule_stream = huffman::encode(&rule_raw, 0);
    let mask_stream = huffman::encode(masks, 0);
    for (label, raw, coded) in [
        ("keys", key_raw.len(), key_stream.len()),
        ("names", name_raw.len(), name_stream.len()),
        ("rules", rule_raw.len(), rule_stream.len()),
        ("masks", masks.len(), mask_stream.len()),
    ] {
        assert!(
            u16::try_from(raw).is_ok() && u16::try_from(coded).is_ok(),
            "{label} stream is {raw} raw / {coded} coded bytes"
        );
    }
    assert!(
        u16::try_from(entries.len()).is_ok() && u16::try_from(literal_names.len()).is_ok(),
        "too many entries or literal names"
    );
    assert!(
        literal_names.len() <= usize::from(MAX_RULE_ARGUMENT) + 1,
        "{} literal names do not fit the 12-bit rule argument (max {})",
        literal_names.len(),
        usize::from(MAX_RULE_ARGUMENT) + 1
    );
    assert!(
        u16::try_from(key_bytes).is_ok(),
        "the key arena is {key_bytes} bytes; the runtime's key offsets are u16"
    );

    let decoded_keys = huffman::decode(&key_stream, key_raw.len(), 1).expect("keys round-trip");
    let decoded_names = huffman::decode(&name_stream, name_raw.len(), 0).expect("names round-trip");
    let decoded_rules = huffman::decode(&rule_stream, rule_raw.len(), 0).expect("rules round-trip");
    let decoded_masks = huffman::decode(&mask_stream, masks.len(), 0).expect("masks round-trip");
    assert_eq!(decoded_keys, key_raw, "key stream round-trip");
    assert_eq!(decoded_names, name_raw, "name stream round-trip");
    assert_eq!(decoded_rules, rule_raw, "rule stream round-trip");
    assert_eq!(decoded_masks, masks, "mask stream round-trip");

    Payload {
        key_raw,
        name_raw,
        rule_raw,
        mask_raw: masks.to_vec(),
        key_stream,
        name_stream,
        rule_stream,
        mask_stream,
        decoded_keys,
        decoded_names,
        tone_codes: pack_bits(entries.iter().map(|entry| entry.tone_code), 3),
        tone_masks: pack_bits(entries.iter().map(|entry| entry.tone_mask), 2),
        key_bytes,
        literal_bytes,
    }
}

/// Writes the section header, descriptors, streams and tone arrays.
fn assemble(entries: usize, literals: usize, payload: &Payload) -> Vec<u8> {
    let mut section = Vec::new();
    section.put_u16_le(u16::try_from(entries).expect("entry count"));
    section.put_u16_le(u16::try_from(literals).expect("literal count"));
    for (raw, coded, order) in [
        (&payload.key_raw, &payload.key_stream, 1u8),
        (&payload.name_raw, &payload.name_stream, 0),
        (&payload.rule_raw, &payload.rule_stream, 0),
        (&payload.mask_raw, &payload.mask_stream, 0),
    ] {
        section.put_u16_le(u16::try_from(raw.len()).expect("raw length"));
        section.put_u16_le(u16::try_from(coded.len()).expect("coded length"));
        section.push(order);
    }
    section.extend_from_slice(&payload.key_stream);
    section.extend_from_slice(&payload.name_stream);
    section.extend_from_slice(&payload.rule_stream);
    section.extend_from_slice(&payload.mask_stream);
    section.extend_from_slice(&payload.tone_codes);
    section.extend_from_slice(&payload.tone_masks);
    section
}

/// Rebuilds every entry from the decoded streams and compares it to the input.
fn verify_streams(
    entries: &[EmojiEntry],
    all_cps: &[Vec<u32>],
    assigned: &[(Rule, u16)],
    decoded_keys: &[u8],
    decoded_names: &[u8],
    ucd: &BTreeMap<u32, String>,
) {
    let decoded_entries = decode_entries(entries.len(), assigned, decoded_keys, decoded_names)
        .expect("decoded streams parse");
    for (index, entry) in entries.iter().enumerate() {
        let (cps, name, rule, arg) = &decoded_entries[index];
        assert_eq!(cps, &all_cps[index], "decoded key {index}");
        let materialized = if *rule == Rule::Literal {
            Some(name.clone())
        } else {
            generated_name(*rule, *arg, cps, entries, ucd)
        };
        assert_eq!(
            materialized.as_deref(),
            Some(entry.name.as_str()),
            "decoded name for {:?}",
            entry.key
        );
        assert_eq!(*rule, assigned[index].0, "decoded rule {index}");
    }
}

/// A decoded entry: `(code points, literal name, rule, argument)`.
type DecodedEntry = (Vec<u32>, String, Rule, u16);

/// Decodes the key and literal-name streams back into entries.
fn decode_entries(
    count: usize,
    assigned: &[(Rule, u16)],
    keys: &[u8],
    names: &[u8],
) -> Option<Vec<DecodedEntry>> {
    let literal_count = assigned
        .iter()
        .filter(|(rule, _)| *rule == Rule::Literal)
        .count();
    let mut literals = Vec::with_capacity(literal_count);
    if literal_count > 0 {
        let mut parts = names.split(|&byte| byte == 0);
        for _ in 0..literal_count {
            literals.push(std::str::from_utf8(parts.next()?).ok()?);
        }
    } else {
        assert!(names.is_empty(), "literal bytes without a literal rule");
    }
    let mut out = Vec::with_capacity(count);
    let mut at = 0usize;
    let mut previous = 0u32;
    for &(rule, arg) in assigned.iter().take(count) {
        let (delta, used) = zeta_unicode_format::read_varint(keys.get(at..)?).expect("a key delta");
        at += used;
        let first = previous.checked_add(delta)?;
        previous = first;
        let (length, used) = zeta_unicode_format::read_varint(keys.get(at..)?)?;
        at += used;
        let length = usize::try_from(length).ok()?;
        let mut cps = Vec::with_capacity(length);
        cps.push(first);
        for _ in 1..length {
            let (cp, used) = zeta_unicode_format::read_varint(keys.get(at..)?)?;
            at += used;
            cps.push(cp);
        }
        let literal = if rule == Rule::Literal {
            literals.get(usize::from(arg))?.to_string()
        } else {
            String::new()
        };
        out.push((cps, literal, rule, arg));
    }
    Some(out)
}

/// Assigns the first grammar rule that reproduces `entry.name`, if any.
///
/// The returned argument is the `ucd_mask` word mask, the literal ordinal, or the gender base
/// candidate (`0` direct, `1` through the `person` base).
fn rule_for(
    entry: &EmojiEntry,
    cps: &[u32],
    entries: &[EmojiEntry],
    ucd: &BTreeMap<u32, String>,
) -> Option<(Rule, u16)> {
    let name = entry.name.as_str();
    if let Some(rule) = single_code_point_rule(cps, name, ucd) {
        return Some(rule);
    }
    if let Some(rule) = gender_rule(cps, name, entries) {
        return Some(rule);
    }
    if let Some(rule) = facing_rule(cps, name, entries) {
        return Some(rule);
    }
    if let Some(rule) = role_rule(cps, name, entries) {
        return Some(rule);
    }
    if let Some(rule) = pair_rule(cps, name) {
        return Some(rule);
    }
    family_rule(cps, name)
}

/// The rules that apply to one or two code points: keycap, tone component, `ucd_mask`, clock.
fn single_code_point_rule(
    cps: &[u32],
    name: &str,
    ucd: &BTreeMap<u32, String>,
) -> Option<(Rule, u16)> {
    if cps.len() == 2
        && cps.get(1) == Some(&0x20E3)
        && keycap_word(cps[0]).is_some_and(|word| name == format!("KEYCAP {word}"))
    {
        return Some((Rule::Keycap, 0));
    }
    if cps.len() != 1 {
        return None;
    }
    let first = cps[0];
    if let Some(index) = TONES.iter().position(|&tone| tone == first)
        && name == TONE_NAMES[index]
    {
        return Some((Rule::ToneComponent, 0));
    }
    if first == 0x1F51F && name == "keycap: 10" {
        return Some((Rule::Keycap10, 0));
    }
    let ucd_name = ucd.get(&first)?;
    if let Some((mask, hyphen)) = ucd_mask(ucd_name, name) {
        return Some((
            if hyphen {
                Rule::UcdMaskHyphen
            } else {
                Rule::UcdMask
            },
            u16::from(mask),
        ));
    }
    clock_name(ucd_name)
        .is_some_and(|clock| clock == name)
        .then_some((Rule::Clock, 0))
}

/// A gender swap with an explicit `ZWJ` plus `U+2640`/`U+2642` suffix.
fn gender_rule(cps: &[u32], name: &str, entries: &[EmojiEntry]) -> Option<(Rule, u16)> {
    let last = *cps.last()?;
    if cps.len() < 3 || cps[cps.len() - 2] != ZWJ || !matches!(last, 0x2640 | 0x2642) {
        return None;
    }
    let gender = if last == 0x2640 { "woman" } else { "man" };
    let rule = if last == 0x2640 {
        Rule::GenderWoman
    } else {
        Rule::GenderMan
    };
    if let Some(base) = find_entry(&cps[..cps.len() - 2], entries) {
        return (swap_person(&entries[base].name, gender) == name).then_some((rule, 0));
    }
    if matches!(cps[0], 0x1F468 | 0x1F469) {
        let mut candidate = vec![0x1F9D1];
        candidate.extend_from_slice(&cps[1..cps.len() - 2]);
        let base = find_entry(&candidate, entries)?;
        return (swap_person(&entries[base].name, gender) == name).then_some((rule, 0));
    }
    None
}

/// A `ZWJ` plus `U+27A1` suffix: the base name plus ` facing right`.
fn facing_rule(cps: &[u32], name: &str, entries: &[EmojiEntry]) -> Option<(Rule, u16)> {
    if cps.len() < 3 || cps[cps.len() - 2] != ZWJ || cps.last() != Some(&0x27A1) {
        return None;
    }
    let base = find_entry(&cps[..cps.len() - 2], entries)?;
    (format!("{} facing right", entries[base].name) == name).then_some((Rule::Facing, 0))
}

/// A three-code-point role whose `person` base supplies the name.
fn role_rule(cps: &[u32], name: &str, entries: &[EmojiEntry]) -> Option<(Rule, u16)> {
    if cps.len() != 3 || cps[1] != ZWJ || cps[0] == 0x1F9D1 {
        return None;
    }
    let gender = people_word(cps[0])?;
    let mut candidate = vec![0x1F9D1];
    candidate.extend_from_slice(&cps[1..]);
    let base = find_entry(&candidate, entries)?;
    (swap_person(&entries[base].name, gender) == name).then_some((
        if cps[0] == 0x1F469 {
            Rule::GenderWoman
        } else {
            Rule::GenderMan
        },
        0,
    ))
}

/// Two people plus an activity: `plural word + pair phrase`.
fn pair_rule(cps: &[u32], name: &str) -> Option<(Rule, u16)> {
    if cps.len() != 5 || cps[1] != ZWJ || cps[3] != ZWJ {
        return None;
    }
    let word = plural(cps[0], cps[4])?;
    let phrase = pair_phrase(cps[2])?;
    (name == format!("{word} {phrase}")).then_some((Rule::Pair, 0))
}

/// Family, couple and kiss: comma-joined family words around a `U+2764`/`U+1F48B` heart.
fn family_rule(cps: &[u32], name: &str) -> Option<(Rule, u16)> {
    if !cps.contains(&ZWJ) {
        return None;
    }
    if cps.iter().all(|&cp| cp == ZWJ || family_word(cp).is_some())
        && name == format!("family: {}", join_family(cps, &[ZWJ]))
    {
        return Some((Rule::Family, 0));
    }
    if !cps.contains(&0x2764) {
        return None;
    }
    let allowed = cps
        .iter()
        .all(|&cp| cp == ZWJ || cp == 0x2764 || cp == 0x1F48B || family_word(cp).is_some());
    if !allowed {
        return None;
    }
    let words = join_family(cps, &[ZWJ, 0x2764, 0x1F48B]);
    if cps.contains(&0x1F48B) {
        return (name == format!("kiss: {words}")).then_some((Rule::Kiss, 0));
    }
    (name == format!("couple with heart: {words}")).then_some((Rule::Couple, 0))
}

/// The trainer's table lookups for the shared emoji rule renderer.
struct BuildRuleContext<'a> {
    /// The canonical entry table, sorted by key.
    entries: &'a [EmojiEntry],
    /// The single-code-point `DerivedName.txt` names.
    ucd: &'a BTreeMap<u32, String>,
}

impl RuleContext for BuildRuleContext<'_> {
    fn ucd_name(&mut self, cp: u32) -> Option<String> {
        self.ucd.get(&cp).cloned()
    }

    fn base_name(&mut self, cps: &[u32]) -> Option<String> {
        find_entry(cps, self.entries)
            .and_then(|index| self.entries.get(index))
            .map(|entry| entry.name.clone())
    }
}

/// Materialises the name a rule produces, or `None` when its base is not in the table.
///
/// [`Rule::Literal`] returns `None`: its name comes from the literal-name arena, not a rule.
fn generated_name(
    rule: Rule,
    arg: u16,
    cps: &[u32],
    entries: &[EmojiEntry],
    ucd: &BTreeMap<u32, String>,
) -> Option<String> {
    rule_name(rule, arg, cps, &mut BuildRuleContext { entries, ucd })
}

/// The index of `cps` in the sorted entry table.
fn find_entry(cps: &[u32], entries: &[EmojiEntry]) -> Option<usize> {
    let mut buffer = [0u8; MAX_KEY];
    let length = encode_cps(cps, &mut buffer)?;
    let key = std::str::from_utf8(&buffer[..length]).ok()?;
    entries
        .binary_search_by(|entry| entry.key.as_str().cmp(key))
        .ok()
}

/// The gender word of a role's leading code point.
const fn people_word(cp: u32) -> Option<&'static str> {
    Some(match cp {
        0x1F9D1 => "person",
        0x1F468 => "man",
        0x1F469 => "woman",
        _ => return None,
    })
}

/// The `ucd_mask` rule: the first mask selecting `name` from `ucd`'s lowercase words.
///
/// This searches with [`ucd_mask_name`] itself, so the accepted mask is guaranteed to render
/// the way the runtime will render it.
fn ucd_mask(ucd: &str, name: &str) -> Option<(u8, bool)> {
    let words = ucd.split(' ').count();
    if words == 0 || words > 8 {
        return None;
    }
    for mask in 1u16..(1u16 << words) {
        let mask = u8::try_from(mask).ok()?;
        if ucd_mask_name(ucd, mask, false) == name {
            return Some((mask, false));
        }
        if ucd_mask_name(ucd, mask, true) == name {
            return Some((mask, true));
        }
    }
    None
}

/// Parses emoji entries with the precedence named sequences (3) > fully-qualified (2) >
/// minimally-qualified (1) > unqualified (0). Emoji 5.0 and 11.0 spell some rows
/// `non-fully-qualified`, which ranks with `unqualified`: it is safe because the
/// fully-qualified variant of the same canonical key outranks it.
fn parse_emoji(emoji_text: &str, named_text: &str) -> BTreeMap<Vec<u32>, (u8, String)> {
    let mut entries: BTreeMap<Vec<u32>, (u8, String)> = BTreeMap::new();
    for (number, line) in records(emoji_text) {
        let (left, rest) = line
            .split_once(';')
            .unwrap_or_else(|| panic!("line {number}: {line}"));
        let (status, comment) = rest.split_once('#').expect("comment");
        let cps = parse_code_points(left, number);
        let rank = match status.trim() {
            "fully-qualified" | "component" => 2u8,
            "minimally-qualified" => 1,
            "unqualified" | "non-fully-qualified" => 0,
            other => panic!("line {number}: unknown status {other}"),
        };
        let words: Vec<&str> = comment.split_whitespace().collect();
        assert!(
            words.len() >= 2,
            "line {number}: unexpected comment: {comment}"
        );
        // Emoji 13.0 added the `E<version>` token; pre-13.0 comments are `<emoji> <name>`.
        let name_at = usize::from(words.get(1).is_some_and(|word| is_emoji_version(word))) + 1;
        keep_best(&mut entries, cps, rank, words[name_at..].join(" "));
    }

    let emoji_keys: std::collections::BTreeSet<Vec<u32>> =
        entries.keys().map(|cps| canonical_cps(cps)).collect();
    for (number, line) in records(named_text) {
        let (name, cps) = line
            .split_once(';')
            .unwrap_or_else(|| panic!("line {number}: {line}"));
        let cps = parse_code_points(cps, number);
        if !emoji_keys.contains(&canonical_cps(&cps)) {
            continue;
        }
        keep_best(&mut entries, cps, 3, name.trim().to_owned());
    }
    entries
}

/// Does `word` look like the `E<major>.<minor>` token in modern `emoji-test.txt` comments?
fn is_emoji_version(word: &str) -> bool {
    let Some(rest) = word.strip_prefix('E') else {
        return false;
    };
    let mut parts = rest.split('.');
    let numeric = |part: Option<&str>| {
        part.is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    numeric(parts.next()) && numeric(parts.next()) && parts.next().is_none()
}

/// The canonical key of a sequence: `U+FE0F` removed.
fn canonical_cps(cps: &[u32]) -> Vec<u32> {
    cps.iter().copied().filter(|&cp| cp != 0xFE0F).collect()
}

/// Keeps the higher-precedence name; a tie keeps the later entry.
fn keep_best(
    entries: &mut BTreeMap<Vec<u32>, (u8, String)>,
    cps: Vec<u32>,
    rank: u8,
    name: String,
) {
    match entries.get(&cps) {
        Some((existing, _)) if *existing > rank => {}
        _ => {
            entries.insert(cps, (rank, name));
        }
    }
}

/// Stores every variant of `group` verbatim, so a group no rule reproduces is not lost.
fn store_verbatim(kept: &mut BTreeMap<Vec<u32>, (String, u8, u8)>, group: &[ToneVariant]) {
    for (_tones, variant_name, full_cps) in group {
        kept.insert(full_cps.clone(), (variant_name.clone(), 0, 0));
    }
}

/// Reconstructs a base entry for a tone-variant group whose base Unicode does not list.
fn derive_base(group: &[ToneVariant]) -> Option<(String, u8)> {
    let (tones, first_name, _) = group.first()?;
    [3u8, 1, 4, 2].into_iter().find_map(|rule| {
        let base = strip_phrase(rule, first_name, tones)?;
        group
            .iter()
            .all(|(tones, name, _)| format_toned(rule, &base, tones) == *name)
            .then_some((base, rule))
    })
}

/// The inverse of [`format_toned`] for one variant: removes the tone phrase `rule` inserts.
fn strip_phrase(rule: u8, name: &str, tones: &[usize]) -> Option<String> {
    let phrase = tone_phrase(rule, tones);
    match rule {
        2 | 4 => name
            .strip_suffix(&format!(", {phrase}"))
            .or_else(|| name.strip_suffix(&format!(": {phrase}")))
            .map(str::to_owned),
        _ => {
            let (head, rest) = name.split_once(": ")?;
            let tail = rest.strip_prefix(&format!("{phrase}, "))?;
            Some(format!("{head}: {tail}"))
        }
    }
}

/// Parses a whitespace-separated list of hexadecimal code points.
fn parse_code_points(text: &str, line: usize) -> Vec<u32> {
    let cps: Vec<u32> = text.split_whitespace().map(hex_u32).collect();
    assert!(!cps.is_empty(), "line {line}: no code points");
    cps
}

/// Parses the code points of a key string.
fn code_points(key: &str) -> Vec<u32> {
    key.chars().map(u32::from).collect()
}

#[cfg(test)]
mod tests {
    use zeta_unicode_format::unpack_bits;

    use super::*;

    const EMOJI: &str = include_str!("../../../zeta-unicode/data/emoji-test.txt");
    const NAMED: &str = include_str!("../../../zeta-unicode/data/NamedSequences.txt");
    const DERIVED: &str = include_str!("../../../zeta-unicode/data/DerivedName.txt");

    /// The canonical emoji table, rebuilt here from the vendored files without calling the
    /// encoder's own parser.
    #[expect(
        clippy::too_many_lines,
        reason = "a direct transcription of the shipped table's tone splitting"
    )]
    fn oracle_entries() -> Vec<EmojiEntry> {
        let mut ranked: BTreeMap<Vec<u32>, (u8, String)> = BTreeMap::new();
        for (number, line) in records(EMOJI) {
            let (left, rest) = line
                .split_once(';')
                .unwrap_or_else(|| panic!("line {number}: {line}"));
            let (status, comment) = rest.split_once('#').expect("comment");
            let cps = parse_code_points(left, number);
            let rank = match status.trim() {
                "fully-qualified" | "component" => 2u8,
                "minimally-qualified" => 1,
                "unqualified" | "non-fully-qualified" => 0,
                other => panic!("line {number}: unknown status {other}"),
            };
            let words: Vec<&str> = comment.split_whitespace().collect();
            let name_at = usize::from(words.get(1).is_some_and(|word| is_emoji_version(word))) + 1;
            let name = words[name_at..].join(" ");
            match ranked.get(&cps) {
                Some((best, _)) if *best > rank => {}
                _ => {
                    ranked.insert(cps, (rank, name));
                }
            }
        }
        let emoji_keys: std::collections::BTreeSet<Vec<u32>> =
            ranked.keys().map(|cps| canonical_cps(cps)).collect();
        for (number, line) in records(NAMED) {
            let (name, cps) = line
                .split_once(';')
                .unwrap_or_else(|| panic!("line {number}: {line}"));
            let cps = parse_code_points(cps, number);
            if !emoji_keys.contains(&canonical_cps(&cps)) {
                continue;
            }
            match ranked.get(&cps) {
                Some((best, _)) if *best > 3 => {}
                _ => {
                    ranked.insert(cps, (3, name.trim().to_owned()));
                }
            }
        }

        let mut canonical: BTreeMap<Vec<u32>, (u8, String)> = BTreeMap::new();
        for (cps, (rank, name)) in &ranked {
            let cps = canonical_cps(cps);
            match canonical.get(&cps) {
                Some((best, _)) if *best > *rank => {}
                _ => {
                    canonical.insert(cps, (*rank, name.clone()));
                }
            }
        }

        let mut kept: BTreeMap<Vec<u32>, (String, u8, u8)> = BTreeMap::new();
        let mut groups: ToneVariants = BTreeMap::new();
        for (cps, (_, name)) in &canonical {
            let tones = tone_positions(cps.iter().copied());
            if tones.is_empty() || cps.len() == 1 {
                kept.insert(cps.clone(), (name.clone(), 0, 0));
            } else {
                let base: Vec<u32> = cps.iter().copied().filter(|cp| !is_tone(*cp)).collect();
                groups
                    .entry(base)
                    .or_default()
                    .push((tones, name.clone(), cps.clone()));
            }
        }
        for (base, group) in &groups {
            if group
                .iter()
                .any(|(tones, _, _)| !(1..=2).contains(&tones.len()))
            {
                for (_tones, name, cps) in group {
                    kept.insert(cps.clone(), (name.clone(), 0, 0));
                }
                continue;
            }
            let (base_name, mut rule) = if let Some((name, _, _)) = kept.get(base) {
                (name.clone(), 0)
            } else {
                let derived = group.first().and_then(|(tones, first, _)| {
                    [3u8, 1, 4, 2].into_iter().find_map(|rule| {
                        let phrase = tone_phrase(rule, tones);
                        let base = match rule {
                            2 | 4 => first
                                .strip_suffix(&format!(", {phrase}"))
                                .or_else(|| first.strip_suffix(&format!(": {phrase}")))?,
                            _ => {
                                let (head, rest) = first.split_once(": ")?;
                                let tail = rest.strip_prefix(&format!("{phrase}, "))?;
                                return Some((format!("{head}: {tail}"), rule));
                            }
                        };
                        Some((base.to_owned(), rule))
                    })
                });
                let Some((derived, derived_rule)) = derived else {
                    for (_tones, name, cps) in group {
                        kept.insert(cps.clone(), (name.clone(), 0, 0));
                    }
                    continue;
                };
                kept.insert(base.clone(), (derived.clone(), derived_rule, 0));
                (derived, derived_rule)
            };
            if rule == 0 {
                for candidate in [3u8, 1, 4, 2] {
                    if group.iter().all(|(tones, variant, _)| {
                        format_toned(candidate, &base_name, tones) == *variant
                    }) {
                        rule = candidate;
                        break;
                    }
                }
            }
            let tone_mask = group.iter().fold(0u8, |mask, (tones, _, _)| {
                mask | match tones.len() {
                    1 => 1,
                    2 => 2,
                    _ => 0,
                }
            });
            if rule == 0 {
                for (_tones, name, cps) in group {
                    kept.insert(cps.clone(), (name.clone(), 0, 0));
                }
                continue;
            }
            let entry = kept.get_mut(base).expect("base entry");
            entry.1 = rule;
            entry.2 = tone_mask;
        }

        let mut entries: Vec<EmojiEntry> = kept
            .into_iter()
            .map(|(cps, (name, tone_code, tone_mask))| EmojiEntry {
                key: cps
                    .iter()
                    .map(|&cp| char::from_u32(cp).expect("scalar value"))
                    .collect(),
                name,
                tone_code,
                tone_mask,
            })
            .collect();
        entries.sort_by(|left, right| left.key.as_bytes().cmp(right.key.as_bytes()));
        entries
    }

    /// A decoded emoji section.
    struct Decoded {
        keys: Vec<Vec<u32>>,
        names: Vec<String>,
        rules: Vec<Rule>,
        arguments: Vec<u16>,
        tone_codes: Vec<u8>,
        tone_masks: Vec<u8>,
    }

    /// Decodes the section layout independently of the runtime crate.
    fn decode(section: &[u8]) -> Decoded {
        let count = usize::from(u16::from_le_bytes([section[0], section[1]]));
        let literal_count = usize::from(u16::from_le_bytes([section[2], section[3]]));
        let mut descriptors = Vec::new();
        let mut at = 4usize;
        for _ in 0..4 {
            let raw = usize::from(u16::from_le_bytes([section[at], section[at + 1]]));
            let coded = usize::from(u16::from_le_bytes([section[at + 2], section[at + 3]]));
            let order = section[at + 4];
            descriptors.push((raw, coded, order));
            at += 5;
        }
        let mut streams = Vec::new();
        for &(raw, coded, order) in &descriptors {
            streams.push(huffman::decode(&section[at..at + coded], raw, order).expect("stream"));
            at += coded;
        }
        let key_raw = &streams[0];
        let name_raw = &streams[1];
        let rule_raw = &streams[2];
        let mask_raw = &streams[3];
        assert_eq!(rule_raw.len(), count);
        let names: Vec<String> = if literal_count == 0 {
            Vec::new()
        } else {
            name_raw
                .split(|&byte| byte == 0)
                .map(|part| std::str::from_utf8(part).expect("UTF-8").to_owned())
                .collect()
        };
        assert_eq!(names.len(), literal_count);
        let mut keys = Vec::with_capacity(count);
        let mut cursor = 0usize;
        let mut previous = 0u32;
        for _ in 0..count {
            let (delta, used) =
                zeta_unicode_format::read_varint(&key_raw[cursor..]).expect("delta");
            cursor += used;
            let first = previous + delta;
            previous = first;
            let (length, used) =
                zeta_unicode_format::read_varint(&key_raw[cursor..]).expect("length");
            cursor += used;
            let length = usize::try_from(length).expect("key length");
            let mut cps = Vec::with_capacity(length);
            cps.push(first);
            for _ in 1..length {
                let (cp, used) =
                    zeta_unicode_format::read_varint(&key_raw[cursor..]).expect("code point");
                cursor += used;
                cps.push(cp);
            }
            keys.push(cps);
        }
        assert_eq!(cursor, key_raw.len());
        let mut rules = Vec::with_capacity(count);
        let mut arguments = Vec::with_capacity(count);
        let mut literal = 0u16;
        let mut mask_at = 0usize;
        for &id in rule_raw {
            let rule = Rule::from_id(id).expect("the rule stream only holds rule ids");
            let argument = match rule {
                Rule::Literal => {
                    let ordinal = literal;
                    literal += 1;
                    ordinal
                }
                Rule::UcdMask | Rule::UcdMaskHyphen => {
                    let mask = u16::from(mask_raw[mask_at]);
                    mask_at += 1;
                    mask
                }
                _ => 0,
            };
            rules.push(rule);
            arguments.push(argument);
        }
        assert_eq!(usize::from(literal), literal_count);
        assert_eq!(mask_at, mask_raw.len());
        let tone_codes_len = (count * 3).div_ceil(8);
        let tone_codes = section[at..at + tone_codes_len].to_vec();
        at += tone_codes_len;
        let tone_masks = section[at..].to_vec();
        let tone_codes = (0..count)
            .map(|index| unpack_bits(&tone_codes, index, 3).expect("a packed tone code"))
            .collect();
        let tone_masks = (0..count)
            .map(|index| unpack_bits(&tone_masks, index, 2).expect("a packed tone mask"))
            .collect();
        Decoded {
            keys,
            names,
            rules,
            arguments,
            tone_codes,
            tone_masks,
        }
    }

    #[test]
    fn encodes_every_canonical_entry_exactly() {
        let oracle = oracle_entries();
        assert_eq!(oracle.len(), 1_944, "canonical entry count");
        assert_eq!(
            oracle.len(),
            canonical_entries(EMOJI, NAMED).len(),
            "the encoder and the test oracle disagree on the entry set"
        );

        let (section, stats) = build(EMOJI, NAMED, DERIVED);
        assert_eq!(stats.entries, 1_944);
        assert_eq!(stats.literals, 603);
        assert_eq!(stats.ruled, 1_341);
        assert_eq!(
            stats.fallbacks, 0,
            "a grammar rule failed to reproduce a name"
        );
        assert_eq!(stats.rule_counts[usize::from(Rule::Literal.id())], 603);
        assert_eq!(stats.rule_counts[usize::from(Rule::UcdMask.id())], 1_099);
        assert_eq!(stats.rule_counts[usize::from(Rule::UcdMaskHyphen.id())], 5);
        assert_eq!(stats.rule_counts[usize::from(Rule::Keycap.id())], 12);
        assert_eq!(stats.rule_counts[usize::from(Rule::GenderWoman.id())], 72);
        assert_eq!(stats.rule_counts[usize::from(Rule::GenderMan.id())], 72);
        assert_eq!(stats.rule_counts[usize::from(Rule::Facing.id())], 18);
        assert_eq!(stats.rule_counts[usize::from(Rule::Family.id())], 29);
        assert_eq!(stats.rule_counts[usize::from(Rule::Couple.id())], 3);
        assert_eq!(stats.rule_counts[usize::from(Rule::Kiss.id())], 3);
        assert_eq!(stats.rule_counts[usize::from(Rule::Pair.id())], 10);
        assert_eq!(stats.rule_counts[usize::from(Rule::ToneComponent.id())], 5);
        assert_eq!(stats.rule_counts[usize::from(Rule::Clock.id())], 12);
        assert_eq!(stats.rule_counts[usize::from(Rule::Keycap10.id())], 1);
        assert!(
            stats.section_bytes <= 15_000,
            "section grew to {}",
            stats.section_bytes
        );

        let decoded = decode(&section);
        assert_eq!(decoded.keys.len(), oracle.len());
        let ucd = ucd_names(DERIVED);
        for (index, expected) in oracle.iter().enumerate() {
            let key: String = decoded.keys[index]
                .iter()
                .map(|&cp| char::from_u32(cp).expect("scalar value"))
                .collect();
            assert_eq!(key, expected.key, "key {index}");
            assert_eq!(
                decoded.tone_codes[index], expected.tone_code,
                "tone code {key:?}"
            );
            assert_eq!(
                decoded.tone_masks[index], expected.tone_mask,
                "tone mask {key:?}"
            );
            let rule = decoded.rules[index];
            let name = if rule == Rule::Literal {
                decoded.names[usize::from(decoded.arguments[index])].clone()
            } else {
                generated_name(
                    rule,
                    decoded.arguments[index],
                    &decoded.keys[index],
                    &oracle,
                    &ucd,
                )
                .unwrap_or_else(|| panic!("rule {rule:?} did not materialize for {key:?}"))
            };
            assert_eq!(name, expected.name, "name for {key:?}");
        }
        for pair in decoded.keys.windows(2) {
            assert!(pair[0] < pair[1], "keys are not sorted");
        }
    }

    #[test]
    fn every_literal_name_is_unique_and_stored_once() {
        let (section, stats) = build(EMOJI, NAMED, DERIVED);
        let decoded = decode(&section);
        assert_eq!(decoded.names.len(), stats.literals);
        let mut seen = std::collections::BTreeSet::new();
        for name in &decoded.names {
            assert!(seen.insert(name.clone()), "duplicate literal {name:?}");
        }
        // The decoded table measures 38,489 B on Unicode 18.0.0. That includes the runtime's
        // 4 B/entry `first_cps` vector (7,776 B) and its 32 B `first_bytes` admission set,
        // which the earlier 32,000 B bound omitted.
        assert!(
            stats.heap_bytes <= 39_000,
            "heap grew to {}",
            stats.heap_bytes
        );
    }

    #[test]
    fn tone_components_and_keycaps_keep_their_published_names() {
        let entries = canonical_entries(EMOJI, NAMED);
        let find = |key: &str| {
            entries
                .iter()
                .find(|entry| entry.key == key)
                .unwrap_or_else(|| panic!("missing {key:?}"))
        };
        assert_eq!(find("🏻").name, "light skin tone");
        assert_eq!(find("🏻").tone_code, 0);
        assert_eq!(find("2⃣").name, "KEYCAP DIGIT TWO");
        assert_eq!(find("🫶").tone_mask, 1);
        assert_eq!(find("🫱‍🫲").tone_mask, 2);
        assert_eq!(find("🚶‍♂").name, "man walking");
    }

    #[test]
    fn non_fully_qualified_status_parses_like_unqualified() {
        // An Emoji 5.0-style file: `non-fully-qualified` rows exist next to every modern
        // status, and the comments use the modern `E<version>` token shape.
        let emoji = "# emoji-test.txt\n\
                     # Version: 5.0\n\
                     263A ; non-fully-qualified # ☺ E5.0 smiling face\n\
                     263A FE0F ; fully-qualified # ☺️ E5.0 smiling face\n\
                     1F600 ; fully-qualified # 😀 E5.0 grinning face\n\
                     1F3FB ; component # 🏻 E5.0 light skin tone\n\
                     1F1E6 1F1E7 ; minimally-qualified # 🇦🇧 E5.0 flag\n\
                     1F1E8 1F1E9 ; unqualified # 🇨🇩 E5.0 flag\n";
        let entries = canonical_entries(emoji, "");
        let find = |key: &str| {
            entries
                .iter()
                .find(|entry| entry.key == key)
                .unwrap_or_else(|| panic!("missing {key:?}"))
        };
        assert_eq!(entries.len(), 5, "one canonical entry per status family");
        // `U+FE0F` is stripped, so the non-fully-qualified and fully-qualified rows of `263A`
        // fold into one entry.
        assert_eq!(find("☺").name, "smiling face");
        assert_eq!(find("😀").name, "grinning face");
        assert_eq!(find("🏻").name, "light skin tone");
        assert_eq!(find("🇦🇧").name, "flag");
        assert_eq!(find("🇨🇩").name, "flag");
    }

    #[test]
    fn ucd_names_are_the_single_code_point_records() {
        let names = ucd_names(DERIVED);
        assert_eq!(
            names.get(&0x41).map(String::as_str),
            Some("LATIN CAPITAL LETTER A")
        );
        assert_eq!(names.get(&0xA9).map(String::as_str), Some("COPYRIGHT SIGN"));
        assert_eq!(
            names.get(&0x1F44D).map(String::as_str),
            Some("THUMBS UP SIGN")
        );
        assert!(
            !names.contains_key(&0x3400),
            "range lines must not be stored"
        );
        assert!(names.len() > 1_400);
    }
}
